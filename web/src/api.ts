export interface SeriesSummary {
  series_uid: string;
  study_uid: string;
  patient_id: string;
  modality: string;
  rows: number;
  cols: number;
  slice_count: number;
  is_volume: boolean;
  hu_calibrated: boolean;
  uniform_spacing: boolean;
  spacing_mm: number | null;
}

export interface SeriesDetail extends SeriesSummary {
  pixel_spacing: [number, number] | null;
  slice_thickness: number | null;
  warnings: string[];
  depths: number[];
}

export interface SliceData {
  rows: number;
  cols: number;
  huCalibrated: boolean;
  pixels: Int16Array;
}

export async function fetchSeriesList(): Promise<SeriesSummary[]> {
  const res = await fetch("/api/series");
  if (!res.ok) throw new Error(`GET /api/series failed: ${res.status}`);
  return res.json();
}

export async function fetchSeriesDetail(seriesUid: string): Promise<SeriesDetail> {
  const res = await fetch(`/api/series/${encodeURIComponent(seriesUid)}`);
  if (!res.ok) throw new Error(`GET /api/series/${seriesUid} failed: ${res.status}`);
  return res.json();
}

export async function fetchSlice(seriesUid: string, ordinal: number): Promise<SliceData> {
  const res = await fetch(
    `/api/series/${encodeURIComponent(seriesUid)}/slices/${ordinal}`
  );
  if (!res.ok) {
    throw new Error(`GET /api/series/${seriesUid}/slices/${ordinal} failed: ${res.status}`);
  }
  const rows = Number(res.headers.get("X-Strata-Rows"));
  const cols = Number(res.headers.get("X-Strata-Cols"));
  const huCalibrated = res.headers.get("X-Strata-HU-Calibrated") === "true";
  const buf = await res.arrayBuffer();
  // Body is raw little-endian i16, row-major; DataView keeps byte order
  // explicit regardless of host endianness.
  const view = new DataView(buf);
  const pixels = new Int16Array(rows * cols);
  for (let i = 0; i < pixels.length; i++) {
    pixels[i] = view.getInt16(i * 2, true);
  }
  return { rows, cols, huCalibrated, pixels };
}
