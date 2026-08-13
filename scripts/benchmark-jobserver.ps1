param(
    [Parameter(Mandatory = $true)]
    [string]$Ninja,
    [int]$Iterations = 1000
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$work = Join-Path $root 'target\benchmark-jobserver'
$knight = Join-Path $work 'ninja.exe'
$makeflags = '--jobserver-auth=10,42'

cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
New-Item -ItemType Directory -Force $work | Out-Null
Copy-Item -Force (Join-Path $root 'target\release\knight.exe') $knight
[System.IO.File]::WriteAllText(
    (Join-Path $work 'build.ninja'),
    "build all: phony`ndefault all`n"
)

$tools = [ordered]@{
    ninja = (Resolve-Path $Ninja).Path
    knight = (Resolve-Path $knight).Path
}
$samples = @{
    ninja = [System.Collections.Generic.List[double]]::new()
    knight = [System.Collections.Generic.List[double]]::new()
}

function Invoke-Tool([string]$Executable) {
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.Arguments = "-C `"$work`""
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['MAKEFLAGS'] = $makeflags
    [void]$start.EnvironmentVariables.Remove('CARGO_MAKEFLAGS')
    [void]$start.EnvironmentVariables.Remove('MFLAGS')
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    [void]$process.Start()
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $timer.Stop()
    if ($process.ExitCode -ne 0) {
        throw "$Executable failed with exit code $($process.ExitCode)"
    }
    [pscustomobject]@{
        ElapsedMs = $timer.Elapsed.TotalMilliseconds
        Stdout = $stdout.Result
        Stderr = $stderr.Result
    }
}

$reference = Invoke-Tool $tools.ninja
$candidate = Invoke-Tool $tools.knight
if ($reference.Stdout -cne $candidate.Stdout -or $reference.Stderr -cne $candidate.Stderr) {
    throw 'jobserver diagnostic output differs from Ninja'
}

foreach ($name in $tools.Keys) {
    for ($warmup = 0; $warmup -lt 3; $warmup++) {
        [void](Invoke-Tool $tools[$name])
    }
}
for ($i = 0; $i -lt $Iterations; $i++) {
    $order = if ($i % 2 -eq 0) { @('ninja', 'knight') } else { @('knight', 'ninja') }
    foreach ($name in $order) {
        $samples[$name].Add((Invoke-Tool $tools[$name]).ElapsedMs)
    }
}

function Summarize([string]$Name) {
    $sorted = $samples[$Name] | Sort-Object
    [pscustomobject]@{
        Tool = $Name
        Iterations = $Iterations
        MedianMs = [math]::Round($sorted[[int]($sorted.Count / 2)], 3)
        MinimumMs = [math]::Round($sorted[0], 3)
        P95Ms = [math]::Round($sorted[[math]::Min($sorted.Count - 1, [int]($sorted.Count * 0.95))], 3)
    }
}

$results = @(Summarize 'ninja'; Summarize 'knight')
$results | Format-Table -AutoSize
$results | ConvertTo-Json | Set-Content (Join-Path $work 'results.json')
