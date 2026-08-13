param(
    [Parameter(Mandatory = $true)]
    [string]$Ninja,
    [int]$Edges = 10000,
    [int]$Iterations = 30,
    [ValidateSet('independent', 'chain')]
    [string]$Shape = 'independent'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$knight = Join-Path $root 'target\release\knight.exe'
$work = Join-Path $root "target\differential-benchmark-$Shape-$Edges"

cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
New-Item -ItemType Directory -Force $work | Out-Null

$manifest = [System.Text.StringBuilder]::new($Edges * 80)
[void]$manifest.Append("rule touch`n  command = cmd /c type nul > `$out`n")
for ($i = 0; $i -lt $Edges; $i++) {
    if ($Shape -eq 'chain' -and $i -gt 0) {
        [void]$manifest.Append("build out/${i}: touch out/$($i - 1)`n")
    } else {
        [void]$manifest.Append("build out/${i}: touch`n")
    }
}
if ($Shape -eq 'independent') {
    [void]$manifest.Append('build all: phony')
    for ($i = 0; $i -lt $Edges; $i++) {
        [void]$manifest.Append(" out/$i")
    }
    [void]$manifest.Append("`ndefault all`n")
} else {
    [void]$manifest.Append("default out/$($Edges - 1)`n")
}
[System.IO.File]::WriteAllText((Join-Path $work 'build.ninja'), $manifest.ToString())
New-Item -ItemType Directory -Force (Join-Path $work 'out') | Out-Null
for ($i = 0; $i -lt $Edges; $i++) {
    [System.IO.File]::WriteAllBytes((Join-Path $work "out/$i"), [byte[]]::new(0))
}

$tools = [ordered]@{
    ninja = (Resolve-Path $Ninja).Path
    knight = (Resolve-Path $knight).Path
}
$samplesByTool = @{
    ninja = [System.Collections.Generic.List[double]]::new()
    knight = [System.Collections.Generic.List[double]]::new()
}

foreach ($name in $tools.Keys) {
    for ($warmup = 0; $warmup -lt 3; $warmup++) {
        & $tools[$name] -C $work 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "$name warmup failed with exit code $LASTEXITCODE" }
    }
}

# Alternate launch order so filesystem-cache and background-system effects do
# not systematically favor the executable measured first.
for ($i = 0; $i -lt $Iterations; $i++) {
    $order = if ($i % 2 -eq 0) { @('ninja', 'knight') } else { @('knight', 'ninja') }
    foreach ($name in $order) {
        $elapsed = (Measure-Command {
            & $tools[$name] -C $work 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "$name failed with exit code $LASTEXITCODE" }
        }).TotalMilliseconds
        $samplesByTool[$name].Add($elapsed)
    }
}

function Summarize-Tool([string]$Name) {
    $sorted = $samplesByTool[$Name] | Sort-Object
    [pscustomobject]@{
        Tool = $Name
        Shape = $Shape
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
$results | ConvertTo-Json | Set-Content (Join-Path $work 'results.json')
