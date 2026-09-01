# Fetch a real sample DICOM study from The Cancer Imaging Archive (TCIA) NBIA
# REST API so a new user can try strata without hand-building curl commands.
#
#   .\scripts\fetch-sample.ps1
#   .\scripts\fetch-sample.ps1 -Size large
#   .\scripts\fetch-sample.ps1 -OutDir data\sample2 -SeriesUid 1.2.3...

param(
    [string]$OutDir = "data/sample",
    [ValidateSet("small", "large")]
    [string]$Size = "small",
    [string]$Collection = "TCGA-LUAD",
    [string]$SeriesUid = "",
    [switch]$Force
)

# Deliberately NOT "Stop" -- see scripts/bench.ps1. PowerShell 5.1 wraps a
# native command's stderr in ErrorRecords (NativeCommandError), which would
# abort this script on perfectly successful .NET calls that happen to write
# to stderr internally. Failures are checked explicitly instead.
$ErrorActionPreference = "Continue"

$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

$base = "https://services.cancerimagingarchive.net/nbia-api/services/v1"

# The known-good default: verified to download and parse, so the default
# path doesn't depend on the archive's series ordering on any given day.
$knownGoodSmallUid = "1.3.6.1.4.1.14519.5.2.1.7777.9002.288863784292986419246212301446"

$bands = @{
    small = @{ Min = 20; Max = 150 }
    large = @{ Min = 500; Max = 2000 }
}

function Fail($msg) {
    Write-Host "ERROR: $msg" -ForegroundColor Red
    exit 1
}

# HttpClient, not Invoke-WebRequest -- see scripts/bench.ps1 for the measured
# 245ms/request overhead. It matters even more here: getImage bodies run into
# the hundreds of megabytes.
Add-Type -AssemblyName System.Net.Http
$client = New-Object System.Net.Http.HttpClient
$client.Timeout = [TimeSpan]::FromMinutes(5)

function Invoke-NbiaGet([string]$url) {
    try {
        $resp = $client.GetAsync($url).GetAwaiter().GetResult()
    } catch {
        Fail "request to $url failed: $($_.Exception.Message)"
    }
    if (-not $resp.IsSuccessStatusCode) {
        Fail "$url returned HTTP $([int]$resp.StatusCode) $($resp.ReasonPhrase)"
    }
    return $resp
}

function Get-JsonFrom([string]$url) {
    $resp = Invoke-NbiaGet $url
    $text = $resp.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    try {
        return $text | ConvertFrom-Json
    } catch {
        Fail "response from $url was not valid JSON (got $($text.Length) bytes)"
    }
}

# ---- pick a series -------------------------------------------------------

$seriesDescription = $null
$sliceCount = $null

if ($SeriesUid) {
    Write-Host "using explicit series UID: $SeriesUid"
    $uid = $SeriesUid
} else {
    $band = $bands[$Size]
    $useKnownGood = $false

    if ($Size -eq "small") {
        Write-Host "checking known-good series is still available..."
        $check = Get-JsonFrom "$base/getSeries?Collection=$Collection&Modality=CT"
        if ($check | Where-Object { $_.SeriesInstanceUID -eq $knownGoodSmallUid }) {
            $useKnownGood = $true
        } else {
            Write-Host "known-good series not listed for $Collection; falling back to discovery"
        }
    }

    if ($useKnownGood) {
        $uid = $knownGoodSmallUid
        $match = $check | Where-Object { $_.SeriesInstanceUID -eq $uid } | Select-Object -First 1
        $seriesDescription = $match.SeriesDescription
        $sliceCount = $match.ImageCount
    } else {
        Write-Host "querying $Collection CT series for a $Size series ($($band.Min)-$($band.Max) slices)..."
        $series = Get-JsonFrom "$base/getSeries?Collection=$Collection&Modality=CT"
        if (-not $series -or $series.Count -eq 0) {
            Fail "no CT series found for collection '$Collection'"
        }
        $candidate = $series |
            Where-Object { [int]$_.ImageCount -ge $band.Min -and [int]$_.ImageCount -le $band.Max } |
            Sort-Object { [int]$_.ImageCount } |
            Select-Object -First 1
        if (-not $candidate) {
            Fail "no series in '$Collection' has an image count between $($band.Min) and $($band.Max)"
        }
        $uid = $candidate.SeriesInstanceUID
        $seriesDescription = $candidate.SeriesDescription
        $sliceCount = $candidate.ImageCount
    }
}

# ---- guard existing output ------------------------------------------------

if (Test-Path $OutDir) {
    $existing = Get-ChildItem -Path $OutDir -Filter "*.dcm" -File -ErrorAction SilentlyContinue
    if ($existing -and $existing.Count -gt 0 -and -not $Force) {
        Fail "'$OutDir' already contains $($existing.Count) .dcm file(s). Pass -Force to replace them."
    }
    if ($existing -and $existing.Count -gt 0 -and $Force) {
        Write-Host "removing $($existing.Count) existing .dcm file(s) from '$OutDir' (-Force)"
        Remove-Item -Path $existing.FullName -Force
    }
}
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

# ---- download --------------------------------------------------------------

$tmpZip = Join-Path $env:TEMP ("strata-sample-" + [guid]::NewGuid().ToString("N") + ".zip")

Write-Host "downloading series $uid..."
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$imgResp = Invoke-NbiaGet "$base/getImage?SeriesInstanceUID=$uid"
$bytes = $imgResp.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()
[System.IO.File]::WriteAllBytes($tmpZip, $bytes)
$sw.Stop()
Write-Host ("downloaded {0:N1} MB in {1:N1}s" -f ($bytes.Length / 1MB), $sw.Elapsed.TotalSeconds)

# A 200 OK carrying an HTML error page or a truncated body is a real failure
# mode from this API. Check the zip magic before handing it to Expand-Archive
# so the error names the actual problem instead of a cryptic unzip failure.
if ($bytes.Length -lt 4 -or $bytes[0] -ne 0x50 -or $bytes[1] -ne 0x4B -or $bytes[2] -ne 0x03 -or $bytes[3] -ne 0x04) {
    Remove-Item $tmpZip -Force -ErrorAction SilentlyContinue
    Fail "the server did not return a zip archive for series '$uid' (got $($bytes.Length) bytes). The series UID may be invalid, or the archive returned an error page."
}

try {
    Expand-Archive -Path $tmpZip -DestinationPath $OutDir -Force
} catch {
    Fail "failed to extract '$tmpZip' into '$OutDir': $($_.Exception.Message)"
} finally {
    Remove-Item $tmpZip -Force -ErrorAction SilentlyContinue
}

$dcmFiles = Get-ChildItem -Path $OutDir -Filter "*.dcm" -File -Recurse
if ($dcmFiles.Count -eq 0) {
    Fail "extraction produced zero .dcm files in '$OutDir'"
}

# Verify at least one file actually looks like DICOM (DICM magic at offset
# 128 for the standard preamble form, or offset 0 for the no-preamble form)
# rather than letting the server discover a bad file later.
$sample = $dcmFiles[0].FullName
$fs = [System.IO.File]::OpenRead($sample)
$buf = New-Object byte[] 132
$read = $fs.Read($buf, 0, 132)
$fs.Close()
$dicm = [System.Text.Encoding]::ASCII.GetBytes("DICM")
$hasPreambleMagic = ($read -eq 132) -and (Compare-Object $buf[128..131] $dicm -SyncWindow 0).Length -eq 0
$hasNoPreambleMagic = ($read -ge 4) -and (Compare-Object $buf[0..3] $dicm -SyncWindow 0).Length -eq 0
if (-not ($hasPreambleMagic -or $hasNoPreambleMagic)) {
    Fail "extracted files do not look like DICOM (no DICM magic found in '$($dcmFiles[0].Name)')"
}

$client.Dispose()

# ---- report -----------------------------------------------------------------

$totalBytes = ($dcmFiles | Measure-Object -Property Length -Sum).Sum
$outFull = (Resolve-Path $OutDir).Path

Write-Host ""
Write-Host "=== fetched ==="
Write-Host "collection : $Collection"
if ($seriesDescription) { Write-Host "series     : $seriesDescription" }
Write-Host "series uid : $uid"
Write-Host ("slices     : {0}" -f $dcmFiles.Count)
Write-Host ("size       : {0:N1} MB" -f ($totalBytes / 1MB))
Write-Host "path       : $outFull"
Write-Host ""
Write-Host "next: cargo run --release -p strata-server -- --data-dir `"$OutDir`""
