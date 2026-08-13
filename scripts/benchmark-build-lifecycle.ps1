param(
    [Parameter(Mandatory = $true)]
    [string]$Ninja,
    [int]$Sources = 128,
    [int]$ShortCommands = 256,
    [int]$DyndepEdges = 1000,
    [int]$Iterations = 10,
    [int]$FastIterations = 50,
    [int]$Jobs = 0
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($Sources -lt 2) { throw 'Sources must be at least 2' }
if ($ShortCommands -lt 1) { throw 'ShortCommands must be positive' }
if ($DyndepEdges -lt 1) { throw 'DyndepEdges must be positive' }
if ($Iterations -lt 3) { throw 'Iterations must be at least 3' }
if ($FastIterations -lt 3) { throw 'FastIterations must be at least 3' }

$root = Split-Path -Parent $PSScriptRoot
$targetRoot = [System.IO.Path]::GetFullPath((Join-Path $root 'target'))
$workRoot = Join-Path $targetRoot 'benchmark-build-lifecycle'
$knight = Join-Path $targetRoot 'release\knight.exe'
$ninjaPath = (Resolve-Path -LiteralPath $Ninja).Path
$clang = (Get-Command clang++ -ErrorAction Stop).Source
$linker = (Get-Command lld-link -ErrorAction Stop).Source

if ($Jobs -le 0) {
    $Jobs = [Environment]::ProcessorCount
}

function Reset-WorkRoot {
    $resolved = [System.IO.Path]::GetFullPath($workRoot)
    $expectedPrefix = $targetRoot.TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($expectedPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove benchmark directory outside target: $resolved"
    }
    if (Test-Path -LiteralPath $resolved) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
    New-Item -ItemType Directory -Path $resolved | Out-Null
}

function Write-Ascii([string]$Path, [string]$Content) {
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.Encoding]::ASCII)
}

function Ninja-CommandPath([string]$Path) {
    return $Path.Replace('\', '/')
}

function New-CppTree([string]$Path, [bool]$UseDepsLog) {
    New-Item -ItemType Directory -Force (Join-Path $Path 'src') | Out-Null
    New-Item -ItemType Directory -Force (Join-Path $Path 'out') | Out-Null
    Write-Ascii (Join-Path $Path 'src\shared.h') "#pragma once`n#define SHARED_VALUE 7`n"

    $manifest = [System.Text.StringBuilder]::new($Sources * 120)
    [void]$manifest.Append("cxx = $(Ninja-CommandPath $clang)`n")
    [void]$manifest.Append("linker = $(Ninja-CommandPath $linker)`n")
    [void]$manifest.Append("rule cxx`n  command = `"`$cxx`" -MMD -MF `$out.d -c `$in -o `$out`n  depfile = `$out.d`n")
    if ($UseDepsLog) {
        [void]$manifest.Append("  deps = gcc`n")
    }
    [void]$manifest.Append("  description = CXX `$out`n")
    [void]$manifest.Append("rule link`n  command = `"`$linker`" /dll /noentry /noimplib /timestamp:0 /out:`$out `$in`n  description = LINK `$out`n")

    $objects = [System.Collections.Generic.List[string]]::new()
    for ($source = 0; $source -lt $Sources; $source++) {
        $stem = 'unit_{0:D4}' -f $source
        Write-Ascii (Join-Path $Path "src\$stem.cc") "#include `"shared.h`"`nextern `"C`" int $stem() { return SHARED_VALUE + $source; }`n// generation 0`n"
        $object = "out/$stem.obj"
        $objects.Add($object)
        [void]$manifest.Append("build $object`: cxx src/$stem.cc`n")
    }
    [void]$manifest.Append("build out/app.dll: link $($objects -join ' ')`n")
    [void]$manifest.Append("default out/app.dll`n")
    Write-Ascii (Join-Path $Path 'build.ninja') $manifest.ToString()
}

function New-ShortCommandTree([string]$Path) {
    New-Item -ItemType Directory -Force (Join-Path $Path 'out') | Out-Null
    Write-Ascii (Join-Path $Path 'input.bin') 'benchmark-input'
    $manifest = [System.Text.StringBuilder]::new($ShortCommands * 70)
    [void]$manifest.Append("rule copy`n  command = cmd /d /c type input.bin `> `$out`n  description = COPY `$out`n")
    for ($edge = 0; $edge -lt $ShortCommands; $edge++) {
        [void]$manifest.Append(('build out/{0:D4}.bin: copy input.bin' -f $edge)).Append("`n")
    }
    [void]$manifest.Append('build all: phony')
    for ($edge = 0; $edge -lt $ShortCommands; $edge++) {
        [void]$manifest.Append((' out/{0:D4}.bin' -f $edge))
    }
    [void]$manifest.Append("`ndefault all`n")
    Write-Ascii (Join-Path $Path 'build.ninja') $manifest.ToString()
}

function New-ManifestTree([string]$Path) {
    New-Item -ItemType Directory -Force $Path | Out-Null
    $manifest = @"
rule regen
  command = cmd /d /c copy /y generated.in generated.ninja >nul
  generator = 1
  restat = 1
build generated.ninja: regen generated.in
include generated.ninja
"@
    $generated = "build all: phony`ndefault all`n# generation 0`n"
    Write-Ascii (Join-Path $Path 'build.ninja') "$manifest`n"
    Write-Ascii (Join-Path $Path 'generated.in') $generated
    Write-Ascii (Join-Path $Path 'generated.ninja') $generated
}

function New-DyndepTree([string]$Path) {
    New-Item -ItemType Directory -Force $Path | Out-Null
    $manifest = [System.Text.StringBuilder]::new($DyndepEdges * 100)
    [void]$manifest.Append("rule existing`n  command = cmd /d /c echo rebuilt>`$out`n  generator = 1`n")
    for ($edge = 0; $edge -lt $DyndepEdges; $edge++) {
        [void]$manifest.Append("build out_$edge`: existing || deps_$edge.dd`n  dyndep = deps_$edge.dd`n")
        Write-Ascii (Join-Path $Path "deps_$edge.dd") "ninja_dyndep_version = 1`nbuild out_$edge`: dyndep`n"
        Write-Ascii (Join-Path $Path "out_$edge") 'existing'
    }
    [void]$manifest.Append('build all: phony')
    for ($edge = 0; $edge -lt $DyndepEdges; $edge++) {
        [void]$manifest.Append(" out_$edge")
    }
    [void]$manifest.Append("`ndefault all`n")
    Write-Ascii (Join-Path $Path 'build.ninja') $manifest.ToString()
}

function Remove-CppOutputs([string]$Path) {
    for ($source = 0; $source -lt $Sources; $source++) {
        $stem = 'unit_{0:D4}' -f $source
        foreach ($suffix in @('obj', 'obj.d')) {
            $file = Join-Path $Path "out\$stem.$suffix"
            if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
        }
    }
    foreach ($relative in @('out\app.dll', 'out\app.lib', 'out\app.exp', '.ninja_log', '.ninja_deps', '.ninja_lock')) {
        $file = Join-Path $Path $relative
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
    }
}

function Remove-ShortOutputs([string]$Path) {
    for ($edge = 0; $edge -lt $ShortCommands; $edge++) {
        $file = Join-Path $Path ('out\{0:D4}.bin' -f $edge)
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
    }
    foreach ($relative in @('.ninja_log', '.ninja_deps', '.ninja_lock')) {
        $file = Join-Path $Path $relative
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
    }
}

$tools = [ordered]@{
    ninja = $ninjaPath
    knight = $knight
}

function Invoke-Build([string]$Tool, [string]$Path, [string[]]$ExtraArgs = @()) {
    $arguments = @('--quiet', '-C', $Path) + $ExtraArgs
    & $tools[$Tool] @arguments 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "$Tool failed in $Path with exit code $LASTEXITCODE"
    }
}

function Assert-SameFile([string]$Scenario, [string]$RelativePath) {
    $ninjaFile = Join-Path (Join-Path $workRoot "$Scenario\ninja") $RelativePath
    $knightFile = Join-Path (Join-Path $workRoot "$Scenario\knight") $RelativePath
    if (-not (Test-Path -LiteralPath $ninjaFile) -or -not (Test-Path -LiteralPath $knightFile)) {
        throw "$Scenario did not produce $RelativePath with both tools"
    }
    $ninjaHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $ninjaFile).Hash
    $knightHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $knightFile).Hash
    if ($ninjaHash -ne $knightHash) {
        throw "$Scenario produced different $RelativePath content"
    }
}

$allSamples = [System.Collections.Generic.List[object]]::new()

function Measure-Scenario(
    [string]$Scenario,
    [string]$Workload,
    [string[]]$BuildArgs,
    [scriptblock]$Prepare,
    [int]$SampleCount = $Iterations
) {
    Write-Host "Benchmarking $Scenario ($Workload, $SampleCount samples per tool)"
    for ($iteration = 0; $iteration -lt $SampleCount; $iteration++) {
        $order = if ($iteration % 2 -eq 0) { @('ninja', 'knight') } else { @('knight', 'ninja') }
        foreach ($tool in $order) {
            $path = Join-Path $workRoot "$Scenario\$tool"
            & $Prepare $path $iteration
            $elapsed = (Measure-Command { Invoke-Build $tool $path $BuildArgs }).TotalMilliseconds
            $allSamples.Add([pscustomobject]@{
                Scenario = $Scenario
                Workload = $Workload
                Iteration = $iteration + 1
                Tool = $tool
                ElapsedMs = [math]::Round($elapsed, 3)
            })
        }
    }
}

function New-ScenarioTrees([string]$Scenario, [scriptblock]$Create) {
    foreach ($tool in $tools.Keys) {
        $path = Join-Path $workRoot "$Scenario\$tool"
        New-Item -ItemType Directory -Force $path | Out-Null
        & $Create $path
    }
}

function Warm-Both([string]$Scenario, [string[]]$BuildArgs, [int]$Count = 1) {
    foreach ($tool in $tools.Keys) {
        $path = Join-Path $workRoot "$Scenario\$tool"
        for ($warmup = 0; $warmup -lt $Count; $warmup++) {
            Invoke-Build $tool $path $BuildArgs
        }
    }
}

Write-Host 'Building Knight release binary'
cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

Reset-WorkRoot
foreach ($tool in $tools.Keys) {
    for ($warmup = 0; $warmup -lt 3; $warmup++) {
        & $tools[$tool] --version 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "$tool --version failed" }
    }
}

$parallelArgs = @('-j', "$Jobs")
$serialArgs = @('-j', '1')

New-ScenarioTrees 'clean-parallel' { param($path) New-CppTree $path $true }
Warm-Both 'clean-parallel' $parallelArgs
foreach ($tool in $tools.Keys) { Remove-CppOutputs (Join-Path $workRoot "clean-parallel\$tool") }
Measure-Scenario 'clean-parallel' "$Sources C++ files, j=$Jobs" $parallelArgs { param($path, $iteration) Remove-CppOutputs $path }
Assert-SameFile 'clean-parallel' 'out\app.dll'

New-ScenarioTrees 'clean-serial' { param($path) New-CppTree $path $true }
Warm-Both 'clean-serial' $serialArgs
foreach ($tool in $tools.Keys) { Remove-CppOutputs (Join-Path $workRoot "clean-serial\$tool") }
Measure-Scenario 'clean-serial' "$Sources C++ files, j=1" $serialArgs { param($path, $iteration) Remove-CppOutputs $path }
Assert-SameFile 'clean-serial' 'out\app.dll'

New-ScenarioTrees 'warm-noop' { param($path) New-CppTree $path $true }
Warm-Both 'warm-noop' $parallelArgs 3
Measure-Scenario 'warm-noop' "$Sources C++ files, no work" $parallelArgs { param($path, $iteration) } $FastIterations
Assert-SameFile 'warm-noop' 'out\app.dll'

New-ScenarioTrees 'single-source' { param($path) New-CppTree $path $true }
Warm-Both 'single-source' $parallelArgs
Measure-Scenario 'single-source' 'one compile and relink' $parallelArgs {
    param($path, $iteration)
    Write-Ascii (Join-Path $path 'src\unit_0000.cc') "#include `"shared.h`"`nextern `"C`" int unit_0000() { return SHARED_VALUE + 0; }`n// generation $($iteration + 1)`n"
}
Assert-SameFile 'single-source' 'out\app.dll'

New-ScenarioTrees 'shared-header' { param($path) New-CppTree $path $true }
Warm-Both 'shared-header' $parallelArgs
Measure-Scenario 'shared-header' "$Sources recompiles and relink" $parallelArgs {
    param($path, $iteration)
    Write-Ascii (Join-Path $path 'src\shared.h') "#pragma once`n#define SHARED_VALUE $($iteration + 8)`n"
}
Assert-SameFile 'shared-header' 'out\app.dll'

New-ScenarioTrees 'depfile-load' { param($path) New-CppTree $path $false }
Warm-Both 'depfile-load' $parallelArgs 3
Measure-Scenario 'depfile-load' "$Sources depfiles, no deps log" $parallelArgs { param($path, $iteration) } $FastIterations
Assert-SameFile 'depfile-load' 'out\app.dll'

New-ScenarioTrees 'short-commands' { param($path) New-ShortCommandTree $path }
Warm-Both 'short-commands' $parallelArgs
foreach ($tool in $tools.Keys) { Remove-ShortOutputs (Join-Path $workRoot "short-commands\$tool") }
Measure-Scenario 'short-commands' "$ShortCommands copies, j=$Jobs" $parallelArgs { param($path, $iteration) Remove-ShortOutputs $path }
Assert-SameFile 'short-commands' ('out\{0:D4}.bin' -f ($ShortCommands - 1))

New-ScenarioTrees 'manifest-regen' { param($path) New-ManifestTree $path }
Warm-Both 'manifest-regen' @()
Measure-Scenario 'manifest-regen' 'regenerate include and reload' @() {
    param($path, $iteration)
    Write-Ascii (Join-Path $path 'generated.in') "build all: phony`ndefault all`n# generation $($iteration + 1)`n"
} $FastIterations
Assert-SameFile 'manifest-regen' 'generated.ninja'

New-ScenarioTrees 'dyndep-load' { param($path) New-DyndepTree $path }
Warm-Both 'dyndep-load' @() 3
Measure-Scenario 'dyndep-load' "$DyndepEdges ready dyndep files" @() { param($path, $iteration) } $FastIterations
Assert-SameFile 'dyndep-load' "out_$($DyndepEdges - 1)"

$summary = [System.Collections.Generic.List[object]]::new()
foreach ($scenario in ($allSamples.Scenario | Select-Object -Unique)) {
    $byScenario = @($allSamples | Where-Object Scenario -eq $scenario)
    $medians = @{}
    $stats = @{}
    foreach ($tool in $tools.Keys) {
        $sorted = @($byScenario | Where-Object Tool -eq $tool | Select-Object -ExpandProperty ElapsedMs | Sort-Object)
        $median = $sorted[[int][math]::Floor($sorted.Count / 2)]
        $stats[$tool] = @{
            Median = $median
            Minimum = $sorted[0]
            P95 = $sorted[[math]::Min($sorted.Count - 1, [int][math]::Floor($sorted.Count * 0.95))]
        }
        $medians[$tool] = $median
    }
    $ratio = $medians.knight / $medians.ninja
    $summary.Add([pscustomobject]@{
        Scenario = $scenario
        Workload = $byScenario[0].Workload
        SamplesPerTool = @($byScenario | Where-Object Tool -eq 'ninja').Count
        NinjaMedianMs = [math]::Round($stats.ninja.Median, 3)
        KnightMedianMs = [math]::Round($stats.knight.Median, 3)
        KnightToNinja = [math]::Round($ratio, 3)
        Winner = if ($ratio -lt 1.0) { 'knight' } else { 'ninja' }
        NinjaMinimumMs = [math]::Round($stats.ninja.Minimum, 3)
        KnightMinimumMs = [math]::Round($stats.knight.Minimum, 3)
        NinjaP95Ms = [math]::Round($stats.ninja.P95, 3)
        KnightP95Ms = [math]::Round($stats.knight.P95, 3)
    })
}

$metadata = [pscustomobject]@{
    Timestamp = (Get-Date).ToString('o')
    Machine = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name).Trim()
    LogicalProcessors = [Environment]::ProcessorCount
    Jobs = $Jobs
    Sources = $Sources
    ShortCommands = $ShortCommands
    DyndepEdges = $DyndepEdges
    Iterations = $Iterations
    FastIterations = $FastIterations
    Ninja = $ninjaPath
    NinjaVersion = (& $ninjaPath --version | Select-Object -First 1)
    Knight = $knight
    KnightVersion = (& $knight --version | Select-Object -First 1)
    Clang = $clang
    Linker = $linker
}

$summary | Format-Table -AutoSize
[pscustomobject]@{
    Metadata = $metadata
    Summary = @($summary)
    Samples = @($allSamples)
} | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $workRoot 'results.json')
$summary | Export-Csv -NoTypeInformation (Join-Path $workRoot 'summary.csv')
Write-Host "Results written to $workRoot"
