param(
    [Parameter(Mandatory = $true)]
    [string]$Ninja,
    [int]$Chains = 3000,
    [int]$Iterations = 50
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$knight = Join-Path $root 'target\release\knight.exe'
$work = Join-Path $root "target\differential-benchmark-pools-$Chains"

cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
New-Item -ItemType Directory -Force $work | Out-Null

$manifest = [System.Text.StringBuilder]::new($Chains * 150)
[void]$manifest.Append("rule run`n  command = cmd /d /c echo `$out`n")
[void]$manifest.Append("pool serial`n  depth = 1`n")
for ($chain = 0; $chain -lt $Chains; $chain++) {
    [void]$manifest.Append("build root/$chain`: run`n")
    [void]$manifest.Append("build pooled/$chain`: run root/$chain`n  pool = serial`n")
    [void]$manifest.Append("build tail/$chain`: run pooled/$chain`n  pool = serial`n")
}
[void]$manifest.Append('build all: phony')
for ($chain = 0; $chain -lt $Chains; $chain++) {
    [void]$manifest.Append(" tail/$chain")
}
[void]$manifest.Append("`ndefault all`n")
[System.IO.File]::WriteAllText((Join-Path $work 'build.ninja'), $manifest.ToString())

$tools = [ordered]@{
    ninja = (Resolve-Path $Ninja).Path
    knight = (Resolve-Path $knight).Path
}
$samplesByTool = @{
    ninja = [System.Collections.Generic.List[double]]::new()
    knight = [System.Collections.Generic.List[double]]::new()
}

function Invoke-PoolPlan([string]$Executable) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.WorkingDirectory = $work
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.Arguments = '-n -j1 --quiet'
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    [void]$process.Start()
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $timer.Stop()
    if ($process.ExitCode -ne 0) {
        throw "$Executable failed with exit code $($process.ExitCode): $($stderr.Result)"
    }
    [pscustomobject]@{
        ElapsedMs = $timer.Elapsed.TotalMilliseconds
        Output = $stdout.Result + $stderr.Result
    }
}

$expected = (Invoke-PoolPlan $tools.ninja).Output
$actual = (Invoke-PoolPlan $tools.knight).Output
if ($actual -cne $expected) {
    throw 'Knight and Ninja produced different quiet pool-plan output'
}

foreach ($name in $tools.Keys) {
    for ($warmup = 0; $warmup -lt 3; $warmup++) {
        [void](Invoke-PoolPlan $tools[$name])
    }
}
for ($iteration = 0; $iteration -lt $Iterations; $iteration++) {
    $order = if ($iteration % 2 -eq 0) { @('ninja', 'knight') } else { @('knight', 'ninja') }
    foreach ($name in $order) {
        $samplesByTool[$name].Add((Invoke-PoolPlan $tools[$name]).ElapsedMs)
    }
}

function Summarize-Tool([string]$Name) {
    $sorted = $samplesByTool[$Name] | Sort-Object
    [pscustomobject]@{
        Tool = $Name
        Chains = $Chains
        Edges = $Chains * 3
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
