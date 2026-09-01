# strata

A browser-native viewer for volumetric medical imaging. Point it at a directory
of DICOM files and it indexes them into patient/study/series, then serves any
series to a WebGL2 viewer with interactive Hounsfield windowing.

No desktop install, no PACS, no upload step. The server reads the files where
they already are.

![Chest CT under a lung window](docs/images/lung-slice38.png)

*TCGA-LUAD chest CT, slice 38/60, lung window (C −600 / W 1500). Rendered in
the browser from a 512×512 int16 Hounsfield texture.*

## Status

**Milestones 1–2 are complete: indexing and 2D slice viewing.**

Working today:

- Recursive DICOM indexing, parsing headers only
- Series grouping with geometrically correct slice ordering
- SQLite index, HTTP API
- WebGL2 slice viewer with shader-side windowing
- Scroll through slices, drag to adjust window centre and width, five radiology presets

Not built yet, and deliberately not implied elsewhere in this README: 3D volume
raymarching, maximum intensity projection, multiplanar reslicing, Hounsfield
volume measurement, and the multi-resolution pyramid with progressive
streaming. Those are milestones 3–6.

## Why the slice ordering matters

Each DICOM file carries its own position and orientation in patient
coordinates. The obvious way to stack slices into a volume is to sort by
`InstanceNumber` — and it is wrong, because that tag is unreliable in
real-world data.

The failure mode is the dangerous kind. A volume built from wrongly ordered
slices still renders successfully. Nothing crashes, no error appears; the
image is simply of anatomy that does not exist in that arrangement.

So ordering is derived from geometry: the slice normal is the cross product of
the row and column direction cosines from `ImageOrientationPatient`, and each
slice's sort key is its `ImagePositionPatient` projected onto that normal.
`InstanceNumber` is never consulted.

The same principle drives the rest of the design:

- **Non-finite values are rejected at the parse boundary.** `ImagePositionPatient`
  is a decimal-string field, and `"nan"` parses to a valid `f64` without error.
  A NaN sort key misplaces exactly one slice while every other slice sorts
  correctly, so non-finite orientations and positions are rejected on read.
- **Hounsfield calibration is never fabricated.** CT values become physically
  meaningful via `hu = raw × RescaleSlope + RescaleIntercept`. When those tags
  are absent the data is not in Hounsfield Units, and `hu_calibrated` is false
  all the way through the API to the UI. Notably `dicom-pixeldata`'s own
  rescale accessor silently substitutes an identity slope and intercept when
  the tags are missing; this project reads tag presence directly instead, so an
  uncalibrated series is reported as uncalibrated rather than quietly presented
  as HU.
- **A corrupt file does not fail a scan.** Per-file errors become warnings
  naming the file. One bad file in a 10,000-file archive must not cost the
  other 9,999.

## Measured performance

Every number below is output from `scripts/bench.ps1`. Nothing here is
estimated.

**Hardware:** AMD Ryzen 9 9950X3D (16C/32T), 93.6 GB RAM, Windows 11 Pro.
**Dataset:** TCGA-LUAD chest CT, 60 slices, 512×512, Implicit VR Little Endian,
Hounsfield calibrated.

| | |
| --- | --- |
| Index 60 slices | **4.8 ms** median of 10 runs |
| Per slice indexed | **0.081 ms** |
| Index rate | **~12,400 slices/sec** |
| Slice fetch, mean | **7.86 ms** |
| Slice fetch, p50 | **6.32 ms** |
| Slice fetch, p99 | **24.35 ms** |
| Slice payload | 524,288 bytes (512 × 512 × int16) |
| Sequential throughput | **~127 slices/sec** |

Indexing is fast because it parses headers only, stopping before `PixelData`;
pixel decoding happens per request. The index figure is measured with a warm
OS file cache, so it reflects parse cost rather than disk cold-start.

A note on how these were measured, because it changed the answer by 25×: an
earlier version of the benchmark used PowerShell's `Invoke-WebRequest` and
reported 254 ms per slice. Nearly all of that was client-side overhead in the
measuring tool. The harness now uses `HttpClient` with a kept-alive connection,
and the result was cross-checked against `curl` (10.1 ms mean / 6.5 ms p50)
before being published.

## Running it

Requires Rust and Node 20+.

```bash
cargo build --release
cd web && npm install && npm run build && cd ..

cargo run --release -p strata-server -- --data-dir /path/to/dicom
```

Then open <http://127.0.0.1:8080>.

| Flag | Default | |
| --- | --- | --- |
| `--data-dir` | *required* | directory to scan recursively |
| `--addr` | `127.0.0.1:8080` | listen address |
| `--index` | `strata.sqlite` | index database path |

### Getting test data

Public, free, no account required — the National Cancer Institute's imaging
archive:

```bash
BASE=https://services.cancerimagingarchive.net/nbia-api/services/v1
curl "$BASE/getSeries?Collection=TCGA-LUAD&Modality=CT" > series.json
curl "$BASE/getImage?SeriesInstanceUID=<uid>" -o series.zip
unzip series.zip -d data/sample
```

## API

| Endpoint | |
| --- | --- |
| `GET /api/health` | status and indexed series count |
| `GET /api/series` | all series with dimensions and quality flags |
| `GET /api/series/:uid` | series detail including per-slice depths |
| `GET /api/series/:uid/slices/:n` | raw little-endian `int16` pixel data |

The slice endpoint returns Hounsfield Units when the series is calibrated and
raw stored values otherwise, with `X-Strata-HU-Calibrated` saying which.
Ordinal `0` is the most inferior slice; ordinals follow the geometric ordering.

## Architecture

```
DICOM directory
      │
      ▼
strata-dicom ──── headers only, no pixel decode
      │           group by SeriesInstanceUID
      │           order by geometric depth
      ▼
strata-server ─── SQLite index, axum HTTP API
      │           pixel decode + Hounsfield rescale on request
      ▼
strata-web ────── int16 texture → isampler2D → windowing in the fragment shader
```

Windowing runs on the GPU against a true `R16I` integer texture rather than a
pre-flattened 8-bit image, so dragging the window is free and the underlying
Hounsfield values stay lossless for the measurement feature in a later
milestone.

## Testing

```bash
cargo test --workspace          # 40 tests
cd web && npx vitest run        # 6 tests
```

Fixtures are valid DICOM files generated programmatically at test time rather
than committed binaries, so each test declares exactly the malformation it
needs — shuffled instance numbers, absent tags, missing preamble, mixed series
in one directory.

Tests that need real imaging data are marked `#[ignore]`:

```bash
cargo test -p strata-dicom --test real_data_test -- --ignored --nocapture
```

## Verification

The screenshots in `docs/images/` were checked against anatomy rather than
merely confirmed to be non-blank. Slice 1 of 60 at −344 mm shows kidneys and
liver; slice 60 at −49 mm shows clavicles, first ribs, and the trachea as an
air-filled circle. Ascending slice order therefore runs inferior to superior,
which is what DICOM patient coordinates require — the ordering guarantee
verified against real anatomy rather than against a fixture.

## License

MIT
