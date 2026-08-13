param(
    [Parameter(Mandatory = $true)]
    [string]$Ninja,
    [int]$Edges = 10000,
    [int]$Iterations = 100
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$knight = Join-Path $root 'target\release\knight.exe'
$workRoot = Join-Path $root "target\benchmark-dyndep-parse-$Edges"

cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) {
    throw "Knight release build failed with exit code $LASTEXITCODE"
}
New-Item -ItemType Directory -Force $workRoot | Out-Null

$manifest = [System.Text.StringBuilder]::new()
$dyndep = [System.Text.StringBuilder]::new("ninja_dyndep_version = 1`n")
[void]$manifest.Append("rule existing`n  command = cmd /d /c echo built>`$out`n  generator = 1`n")
for ($edge = 0; $edge -lt $Edges; $edge++) {
    [void]$manifest.Append("build out_$edge`: existing || deps.dd`n  dyndep = deps.dd`n")
    [void]$dyndep.Append("build out_$edge`: dyndep`n")
}
[void]$manifest.Append('build all: phony')
for ($edge = 0; $edge -lt $Edges; $edge++) {
    [void]$manifest.Append(" out_$edge")
}
[void]$manifest.Append("`ndefault all`n")

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
    [System.IO.File]::WriteAllText((Join-Path $work 'build.ninja'), $manifest.ToString())
    [System.IO.File]::WriteAllText((Join-Path $work 'deps.dd'), $dyndep.ToString())
    for ($edge = 0; $edge -lt $Edges; $edge++) {
        [System.IO.File]::WriteAllText((Join-Path $work "out_$edge"), 'existing')
    }
    for ($warmup = 0; $warmup -lt 3; $warmup++) {
        & $tools[$name] -C $work --quiet 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "$name warmup failed with exit code $LASTEXITCODE"
        }
    }
}

for ($iteration = 0; $iteration -lt $Iterations; $iteration++) {
    $order = if ($iteration % 2 -eq 0) { @('ninja', 'knight') } else { @('knight', 'ninja') }
    foreach ($name in $order) {
        $work = Join-Path $workRoot $name
        $elapsed = (Measure-Command {
            & $tools[$name] -C $work --quiet 2>&1 | Out-Null
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
        Edges = $Edges
        Iterations = $Iterations
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
