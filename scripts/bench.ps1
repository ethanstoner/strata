# Benchmark harness for strata.
#
# Every performance number in README.md comes from this script. If a number is
# not in this script's output, it does not go in the README.
#
#   .\scripts\bench.ps1 -DataDir data\sample

param(
    [string]$DataDir = "data\sample",
    [int]$Port = 8099,
    [int]$Requests = 200
)

# Deliberately NOT "Stop". PowerShell 5.1 wraps a native command's stderr lines
# in ErrorRecords (NativeCommandError), so cargo's progress output would abort
# the script on a perfectly successful build. Native failures are caught by
# explicit $LASTEXITCODE checks instead.
$ErrorActionPreference = "Continue"
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

if (-not (Test-Path $DataDir)) {
    throw "Data directory '$DataDir' not found. Fetch a DICOM series first."
}

$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
$ram = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)
Write-Host "hardware : $cpu, $ram GB RAM"
Write-Host "os       : $((Get-CimInstance Win32_OperatingSystem).Caption)"
Write-Host ""

Write-Host "building release..."
cargo build --release -p strata-server *> $null
if ($LASTEXITCODE -ne 0) { throw "release build failed" }

Write-Host "=== indexing ==="
cargo test --release -p strata-dicom --test bench_test -- --ignored --nocapture |
    Select-String -Pattern "INDEX|median|per_slice"

$db = Join-Path $env:TEMP "strata-bench.sqlite"
if (Test-Path $db) { Remove-Item $db -Force }

$exe = Join-Path $repo "target\release\strata-server.exe"
$proc = Start-Process -FilePath $exe `
    -ArgumentList @("--data-dir", $DataDir, "--addr", "127.0.0.1:$Port", "--index", $db) `
    -PassThru -WindowStyle Hidden

try {
    $base = "http://127.0.0.1:$Port"
    $ready = $false
    foreach ($i in 1..40) {
        try {
            Invoke-RestMethod "$base/api/health" -TimeoutSec 2 | Out-Null
            $ready = $true
            break
        } catch { Start-Sleep -Milliseconds 250 }
    }
    if (-not $ready) { throw "server did not become healthy" }

    $series = Invoke-RestMethod "$base/api/series"
    $uid = $series[0].series_uid
    $count = $series[0].slice_count
    Write-Host ""
    Write-Host "=== slice endpoint ==="
    Write-Host "series   : $count slices, $($series[0].rows)x$($series[0].cols), hu_calibrated=$($series[0].hu_calibrated)"

    # Deliberately NOT Invoke-WebRequest. It adds roughly 245ms of client-side
    # overhead per call and opens a fresh connection each time, which measured
    # this endpoint at 254ms when curl with keep-alive measured 6.5ms p50 --
    # a 25x error attributable entirely to the measuring tool. HttpClient keeps
    # the connection alive and does not parse the body, so what is timed is the
    # server rather than the harness.
    Add-Type -AssemblyName System.Net.Http
    $client = New-Object System.Net.Http.HttpClient
    $client.Timeout = [TimeSpan]::FromSeconds(10)

    foreach ($i in 0..([Math]::Min(9, $count - 1))) {
        $client.GetByteArrayAsync("$base/api/series/$uid/slices/$i").GetAwaiter().GetResult() | Out-Null
    }

    $times = New-Object System.Collections.Generic.List[double]
    $bytes = 0
    foreach ($i in 1..$Requests) {
        $ordinal = ($i - 1) % $count
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $body = $client.GetByteArrayAsync("$base/api/series/$uid/slices/$ordinal").GetAwaiter().GetResult()
        $sw.Stop()
        $times.Add($sw.Elapsed.TotalMilliseconds)
        $bytes = $body.Length
    }
    $client.Dispose()

    $sorted = $times | Sort-Object
    $mean = ($times | Measure-Object -Average).Average
    $p50 = $sorted[[int]($sorted.Count * 0.50)]
    $p99 = $sorted[[int]($sorted.Count * 0.99)]

    Write-Host ("requests : {0}" -f $Requests)
    Write-Host ("payload  : {0} bytes/slice" -f $bytes)
    Write-Host ("mean     : {0:N2} ms" -f $mean)
    Write-Host ("p50      : {0:N2} ms" -f $p50)
    Write-Host ("p99      : {0:N2} ms" -f $p99)
    Write-Host ("min/max  : {0:N2} / {1:N2} ms" -f $sorted[0], $sorted[$sorted.Count - 1])
    Write-Host ("throughput: {0:N0} slices/sec sequential" -f (1000.0 / $mean))
}
finally {
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    if (Test-Path $db) { Remove-Item $db -Force -ErrorAction SilentlyContinue }
}
