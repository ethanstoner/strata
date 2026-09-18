#!/usr/bin/env bash
# Fetch a real sample DICOM study from The Cancer Imaging Archive (TCIA) NBIA
# REST API so a new user can try strata without hand-building curl commands.
# Behaviourally equivalent to fetch-sample.ps1.
#
#   ./scripts/fetch-sample.sh
#   ./scripts/fetch-sample.sh --size large
#   ./scripts/fetch-sample.sh --out-dir data/sample2 --series-uid 1.2.3...

set -u

OUT_DIR="data/sample"
SIZE="small"
COLLECTION="TCGA-LUAD"
SERIES_UID=""
FORCE=0

while [ $# -gt 0 ]; do
    case "$1" in
        --out-dir) OUT_DIR="$2"; shift 2 ;;
        --size) SIZE="$2"; shift 2 ;;
        --collection) COLLECTION="$2"; shift 2 ;;
        --series-uid) SERIES_UID="$2"; shift 2 ;;
        --force) FORCE=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

if [ "$SIZE" != "small" ] && [ "$SIZE" != "large" ]; then
    echo "ERROR: --size must be 'small' or 'large' (got '$SIZE')" >&2
    exit 1
fi

for cmd in curl unzip python3; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "ERROR: required command '$cmd' not found on PATH" >&2
        exit 1
    fi
done

# Being on PATH is not enough: Windows ships a python3 shim that resolves and
# then fails on every call. Without this check its failure surfaces later as a
# bogus "no series matched" message from the series query below.
if ! python3 -c "" >/dev/null 2>&1; then
    echo "ERROR: 'python3' is on PATH but is not a working interpreter" >&2
    exit 1
fi

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 1

base="https://services.cancerimagingarchive.net/nbia-api/services/v1"

# The known-good default: verified to download and parse, so the default
# path doesn't depend on the archive's series ordering on any given day.
known_good_small_uid="1.3.6.1.4.1.14519.5.2.1.7777.9002.288863784292986419246212301446"

case "$SIZE" in
    small) band_min=20;  band_max=150 ;;
    large) band_min=500; band_max=2000 ;;
esac

fail() {
    echo "ERROR: $1" >&2
    exit 1
}

# curl a URL to a file, checking HTTP status explicitly rather than trusting
# a bare exit code -- a non-200 response with a body still "succeeds" as far
# as curl -f alone reports on some servers, and we want the URL in the message.
http_get() {
    url="$1"
    out="$2"
    status=$(curl -s -o "$out" -w "%{http_code}" --max-time 300 "$url" 2>/tmp/strata-curl-err.$$) || {
        err=$(cat /tmp/strata-curl-err.$$ 2>/dev/null)
        rm -f /tmp/strata-curl-err.$$
        fail "request to $url failed: $err"
    }
    rm -f /tmp/strata-curl-err.$$
    if [ "$status" != "200" ]; then
        fail "$url returned HTTP $status"
    fi
}

series_description=""
slice_count=""

if [ -n "$SERIES_UID" ]; then
    echo "using explicit series UID: $SERIES_UID"
    uid="$SERIES_UID"
else
    tmp_series=$(mktemp)
    echo "querying $COLLECTION CT series..."
    http_get "$base/getSeries?Collection=$COLLECTION&Modality=CT" "$tmp_series"

    use_known_good=0
    if [ "$SIZE" = "small" ]; then
        if python3 -c "
import json, sys
data = json.load(open('$tmp_series'))
sys.exit(0 if any(s.get('SeriesInstanceUID') == '$known_good_small_uid' for s in data) else 1)
" ; then
            use_known_good=1
        else
            echo "known-good series not listed for $COLLECTION; falling back to discovery"
        fi
    fi

    if [ "$use_known_good" = "1" ]; then
        uid="$known_good_small_uid"
        info=$(python3 -c "
import json
data = json.load(open('$tmp_series'))
m = next(s for s in data if s.get('SeriesInstanceUID') == '$known_good_small_uid')
print(m.get('SeriesDescription',''))
print(m.get('ImageCount',''))
")
        series_description=$(echo "$info" | sed -n '1p')
        slice_count=$(echo "$info" | sed -n '2p')
    else
        echo "picking a $SIZE series ($band_min-$band_max slices)..."
        result=$(python3 -c "
import json
data = json.load(open('$tmp_series'))
cands = [s for s in data if $band_min <= int(s.get('ImageCount', 0)) <= $band_max]
if not cands:
    raise SystemExit(1)
cands.sort(key=lambda s: int(s['ImageCount']))
best = cands[0]
print(best['SeriesInstanceUID'])
print(best.get('SeriesDescription',''))
print(best.get('ImageCount',''))
") || fail "no series in '$COLLECTION' has an image count between $band_min and $band_max"
        uid=$(echo "$result" | sed -n '1p')
        series_description=$(echo "$result" | sed -n '2p')
        slice_count=$(echo "$result" | sed -n '3p')
    fi
    rm -f "$tmp_series"
fi

if [ -d "$OUT_DIR" ]; then
    existing_count=$(find "$OUT_DIR" -maxdepth 1 -name "*.dcm" -type f 2>/dev/null | wc -l | tr -d ' ')
    if [ "$existing_count" -gt 0 ] && [ "$FORCE" != "1" ]; then
        fail "'$OUT_DIR' already contains $existing_count .dcm file(s). Pass --force to replace them."
    fi
    if [ "$existing_count" -gt 0 ] && [ "$FORCE" = "1" ]; then
        echo "removing $existing_count existing .dcm file(s) from '$OUT_DIR' (--force)"
        find "$OUT_DIR" -maxdepth 1 -name "*.dcm" -type f -delete
    fi
fi
mkdir -p "$OUT_DIR"

tmp_zip=$(mktemp /tmp/strata-sample-XXXXXX.zip)

echo "downloading series $uid..."
start=$(date +%s)
http_get "$base/getImage?SeriesInstanceUID=$uid" "$tmp_zip"
end=$(date +%s)
size_bytes=$(wc -c < "$tmp_zip" | tr -d ' ')
size_mb=$(python3 -c "print(f'{$size_bytes / 1024 / 1024:.1f}')")
echo "downloaded ${size_mb} MB in $((end - start))s"

# A 200 OK carrying an HTML error page or a truncated body is a real failure
# mode from this API. Check the zip magic before handing it to unzip so the
# error names the actual problem instead of a cryptic unzip failure.
magic=$(head -c 4 "$tmp_zip" | od -An -tx1 | tr -d ' \n')
if [ "$magic" != "504b0304" ]; then
    rm -f "$tmp_zip"
    fail "the server did not return a zip archive for series '$uid' (got $size_bytes bytes). The series UID may be invalid, or the archive returned an error page."
fi

unzip -q -o "$tmp_zip" -d "$OUT_DIR" || fail "failed to extract '$tmp_zip' into '$OUT_DIR'"
rm -f "$tmp_zip"

dcm_count=$(find "$OUT_DIR" -name "*.dcm" -type f | wc -l | tr -d ' ')
if [ "$dcm_count" -eq 0 ]; then
    fail "extraction produced zero .dcm files in '$OUT_DIR'"
fi

# Verify at least one file actually looks like DICOM (DICM magic at offset
# 128 for the standard preamble form, or offset 0 for the no-preamble form)
# rather than letting the server discover a bad file later.
sample_file=$(find "$OUT_DIR" -name "*.dcm" -type f | head -n 1)
magic_128=$(dd if="$sample_file" bs=1 skip=128 count=4 2>/dev/null | tr -d '\0')
magic_0=$(dd if="$sample_file" bs=1 count=4 2>/dev/null | tr -d '\0')
if [ "$magic_128" != "DICM" ] && [ "$magic_0" != "DICM" ]; then
    fail "extracted files do not look like DICOM (no DICM magic found in '$sample_file')"
fi

total_bytes=$(find "$OUT_DIR" -name "*.dcm" -type f -exec wc -c {} + | tail -n 1 | awk '{print $1}')
total_mb=$(python3 -c "print(f'{$total_bytes / 1024 / 1024:.1f}')")
out_full=$(cd "$OUT_DIR" && pwd)

echo ""
echo "=== fetched ==="
echo "collection : $COLLECTION"
[ -n "$series_description" ] && echo "series     : $series_description"
echo "series uid : $uid"
echo "slices     : $dcm_count"
echo "size       : ${total_mb} MB"
echo "path       : $out_full"
echo ""
echo "next: cargo run --release -p strata-server -- --data-dir \"$OUT_DIR\""
