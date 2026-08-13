param(
    [Parameter(Mandatory = $true)]
    [string]$Ninja,
    [int]$Files = 1000,
    [int]$Iterations = 50
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$knight = Join-Path $root 'target\release\knight.exe'
$work = Join-Path $root "target\differential-benchmark-includes-$Files"
$parts = Join-Path $work 'parts'

cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw "Knight release build failed with exit code $LASTEXITCODE" }
if (Test-Path -LiteralPath $work) {
    [System.IO.Directory]::Delete($work, $true)
}
New-Item -ItemType Directory -Force $parts | Out-Null

$manifest = [System.Text.StringBuilder]::new($Files * 30)
for ($i = 0; $i -lt $Files; $i++) {
    [void]$manifest.Append("include parts/$i.ninja`n")
    [System.IO.File]::WriteAllText(
        (Join-Path $parts "$i.ninja"),
        "included_$i = value`n"
    )
}
[void]$manifest.Append("build all: phony`ndefault all`n")
[System.IO.File]::WriteAllText((Join-Path $work 'build.ninja'), $manifest.ToString())

$tools = [ordered]@{
    ninja = (Resolve-Path $Ninja).Path
    knight = (Resolve-Path $knight).Path
}
$arguments = @('-C', $work, '-t', 'targets', 'all')
$expected = & $tools.ninja @arguments 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { throw "Ninja validation failed with exit code $LASTEXITCODE" }
$actual = & $tools.knight @arguments 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { throw "Knight validation failed with exit code $LASTEXITCODE" }
if ($actual -ne $expected) { throw 'Target output differs from Ninja' }

$samplesByTool = @{
    ninja = [System.Collections.Generic.List[double]]::new()
    knight = [System.Collections.Generic.List[double]]::new()
}
foreach ($name in $tools.Keys) {
    for ($warmup = 0; $warmup -lt 3; $warmup++) {
        & $tools[$name] @arguments 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "$name warmup failed with exit code $LASTEXITCODE" }
    }
}
for ($i = 0; $i -lt $Iterations; $i++) {
    $order = if ($i % 2 -eq 0) { @('ninja', 'knight') } else { @('knight', 'ninja') }
    foreach ($name in $order) {
        $elapsed = (Measure-Command {
            & $tools[$name] @arguments 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "$name failed with exit code $LASTEXITCODE" }
        }).TotalMilliseconds
        $samplesByTool[$name].Add($elapsed)
    }
}

$results = foreach ($name in $tools.Keys) {
    $sorted = $samplesByTool[$name] | Sort-Object
    [pscustomobject]@{
        Tool = $name
        Files = $Files
        Iterations = $Iterations
        MedianMs = [math]::Round($sorted[[int]($sorted.Count / 2)], 3)
        MinimumMs = [math]::Round($sorted[0], 3)
        P95Ms = [math]::Round(
            $sorted[[math]::Min($sorted.Count - 1, [int]($sorted.Count * 0.95))],
            3
        )
    }
}
$results | Format-Table -AutoSize
$results | ConvertTo-Json | Set-Content (Join-Path $work 'results.json')
