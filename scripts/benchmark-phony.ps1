param(
    [Parameter(Mandatory = $true)]
    [string]$Ninja,
    [int]$Edges = 10000,
    [int]$Iterations = 100
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$knight = Join-Path $root 'target\release\knight.exe'
$work = Join-Path $root "target\differential-benchmark-phony-$Edges"

cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
New-Item -ItemType Directory -Force $work | Out-Null

$manifest = [System.Text.StringBuilder]::new($Edges * 32)
[void]$manifest.Append("build node/0: phony`n")
for ($edge = 1; $edge -lt $Edges; $edge++) {
    [void]$manifest.Append("build node/$edge`: phony node/$($edge - 1)`n")
}
[void]$manifest.Append("default node/$($Edges - 1)`n")
[System.IO.File]::WriteAllText((Join-Path $work 'build.ninja'), $manifest.ToString())

$tools = [ordered]@{
    ninja = (Resolve-Path $Ninja).Path
    knight = (Resolve-Path $knight).Path
}
$samplesByTool = @{
    ninja = [System.Collections.Generic.List[double]]::new()
    knight = [System.Collections.Generic.List[double]]::new()
}

function Invoke-Phony([string]$Executable) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.WorkingDirectory = $work
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
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
        Output = $stdout.Result.Replace("`r`n", "`n").Replace('ninja:', 'tool:').Replace('knight:', 'tool:')
    }
}

$expected = (Invoke-Phony $tools.ninja).Output
$actual = (Invoke-Phony $tools.knight).Output
if ($actual -cne $expected) {
    throw 'Knight and Ninja produced different phony-chain output'
}

foreach ($name in $tools.Keys) {
    for ($warmup = 0; $warmup -lt 3; $warmup++) {
        [void](Invoke-Phony $tools[$name])
    }
}
for ($iteration = 0; $iteration -lt $Iterations; $iteration++) {
    $order = if ($iteration % 2 -eq 0) { @('ninja', 'knight') } else { @('knight', 'ninja') }
    foreach ($name in $order) {
        $samplesByTool[$name].Add((Invoke-Phony $tools[$name]).ElapsedMs)
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
$results | ConvertTo-Json | Set-Content (Join-Path $work 'results.json')
