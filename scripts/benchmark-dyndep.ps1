param(
    [Parameter(Mandatory = $true)]
    [string]$Ninja,
    [int]$Iterations = 50,
    [int]$DelayMs = 100
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$knight = Join-Path $root 'target\release\knight.exe'
$workRoot = Join-Path $root 'target\differential-benchmark-dyndep'

cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
New-Item -ItemType Directory -Force $workRoot | Out-Null

$manifest = @"
rule independent
  command = powershell -NoProfile -Command "Start-Sleep -Milliseconds $DelayMs; Set-Content out1 built; Set-Content out1.imp built"
rule scan
  command = powershell -NoProfile -Command "Start-Sleep -Milliseconds $DelayMs; Copy-Item zdd-in zdd"
rule copy
  command = cmd /d /c copy /y `$in `$out `>nul
build out1 | out1.imp: independent
build zdd: scan zdd-in
build out2: copy out1 || zdd
  dyndep = zdd
default out1 out2
"@
$dyndep = "ninja_dyndep_version = 1`nbuild out2: dyndep | out1.imp`n"
$tools = [ordered]@{
    ninja = (Resolve-Path $Ninja).Path
    knight = (Resolve-Path $knight).Path
}
$samplesByTool = @{
    ninja = [System.Collections.Generic.List[double]]::new()
    knight = [System.Collections.Generic.List[double]]::new()
}

foreach ($name in $tools.Keys) {
    $work = Join-Path $workRoot $name
    New-Item -ItemType Directory -Force $work | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $work 'build.ninja'), "$manifest`n")
    [System.IO.File]::WriteAllText((Join-Path $work 'zdd-in'), $dyndep)
}

$generated = @('out1', 'out1.imp', 'out2', 'zdd', '.ninja_log', '.ninja_deps', '.ninja_lock')
for ($iteration = 0; $iteration -lt $Iterations; $iteration++) {
    $order = if ($iteration % 2 -eq 0) { @('ninja', 'knight') } else { @('knight', 'ninja') }
    foreach ($name in $order) {
        $work = Join-Path $workRoot $name
        foreach ($file in $generated) {
            $path = Join-Path $work $file
            if (Test-Path -LiteralPath $path) {
                Remove-Item -LiteralPath $path -Force
            }
        }
        $elapsed = (Measure-Command {
            & $tools[$name] -C $work -j2 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) {
                throw "$name failed with exit code $LASTEXITCODE"
            }
        }).TotalMilliseconds
        $samplesByTool[$name].Add($elapsed)
    }
}

function Summarize-Tool([string]$Name) {
    $sorted = $samplesByTool[$Name] | Sort-Object
    [pscustomobject]@{
        Tool = $Name
        Iterations = $Iterations
        DelayMs = $DelayMs
        MedianMs = [math]::Round($sorted[[int]($sorted.Count / 2)], 3)
        MinimumMs = [math]::Round($sorted[0], 3)
        P95Ms = [math]::Round($sorted[[math]::Min($sorted.Count - 1, [int]($sorted.Count * 0.95))], 3)
    }
}

$results = @(
    Summarize-Tool 'ninja'
    Summarize-Tool 'knight'
)
$results | Format-Table -AutoSize
$results | ConvertTo-Json | Set-Content (Join-Path $workRoot 'results.json')
