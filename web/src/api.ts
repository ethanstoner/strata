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
  series_description: string | null;
  study_description: string | null;
}

/** Thrown for a non-2xx API response; `message` is the server's own reason text when it sent one (e.g. a 400's plain-text body). */
export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number
  ) {
    super(message);
    this.name = "ApiError";
  }
}

async function errorFor(res: Response, fallback: string): Promise<ApiError> {
  const body = await res.text().catch(() => "");
  return new ApiError(body.trim() || fallback, res.status);
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
  if (!res.ok) throw await errorFor(res, `GET /api/series failed: ${res.status}`);
  return res.json();
}

export async function fetchSeriesDetail(seriesUid: string): Promise<SeriesDetail> {
  const res = await fetch(`/api/series/${encodeURIComponent(seriesUid)}`);
  if (!res.ok) throw await errorFor(res, `GET /api/series/${seriesUid} failed: ${res.status}`);
  return res.json();
}

export interface VolumeData {
  dimX: number;
  dimY: number;
  dimZ: number;
  spacingX: number;
  spacingY: number;
  spacingZ: number;
  huCalibrated: boolean;
  level: number;
  huMin: number;
  huMax: number;
  voxels: Int16Array;
}

export async function fetchVolume(seriesUid: string, level = 1): Promise<VolumeData> {
  const res = await fetch(
    `/api/series/${encodeURIComponent(seriesUid)}/volume?level=${level}`
  );
  if (!res.ok) {
    throw await errorFor(res, `GET /api/series/${seriesUid}/volume?level=${level} failed: ${res.status}`);
  }
  const dimX = Number(res.headers.get("X-Strata-Dim-X"));
  const dimY = Number(res.headers.get("X-Strata-Dim-Y"));
  const dimZ = Number(res.headers.get("X-Strata-Dim-Z"));
  const spacingX = Number(res.headers.get("X-Strata-Spacing-X"));
  const spacingY = Number(res.headers.get("X-Strata-Spacing-Y"));
  const spacingZ = Number(res.headers.get("X-Strata-Spacing-Z"));
  const huCalibrated = res.headers.get("X-Strata-HU-Calibrated") === "true";
  const respLevel = Number(res.headers.get("X-Strata-Level"));
  const huMin = Number(res.headers.get("X-Strata-HU-Min"));
  const huMax = Number(res.headers.get("X-Strata-HU-Max"));
  const buf = await res.arrayBuffer();
  // Same wire format as slices: raw little-endian i16, but volume-ordered
  // x fastest, then y, then z (exactly what texImage3D wants).
  const view = new DataView(buf);
  const voxels = new Int16Array(dimX * dimY * dimZ);
  for (let i = 0; i < voxels.length; i++) {
    voxels[i] = view.getInt16(i * 2, true);
  }
  return {
    dimX,
    dimY,
    dimZ,
    spacingX,
    spacingY,
    spacingZ,
    huCalibrated,
    level: respLevel,
    huMin,
    huMax,
    voxels,
  };
}

export async function fetchSlice(seriesUid: string, ordinal: number): Promise<SliceData> {
  const res = await fetch(
    `/api/series/${encodeURIComponent(seriesUid)}/slices/${ordinal}`
  );
  if (!res.ok) {
    throw await errorFor(res, `GET /api/series/${seriesUid}/slices/${ordinal} failed: ${res.status}`);
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
