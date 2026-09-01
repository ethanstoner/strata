// Pure label-building for the series dropdown, kept separate from main.ts so
// it's unit testable without a DOM.

export interface SeriesLabelInput {
  patient_id: string;
  series_description?: string | null;
  modality: string;
  slice_count: number;
}

/**
 * `PatientID — SeriesDescription — Modality (N slices)` when the scanner
 * recorded a series description, else `PatientID — Modality (N slices)`.
 * Never fabricates a placeholder for a null/empty description — an absent
 * value means the scanner recorded nothing, not "Series 1" or "Unknown".
 */
export function formatSeriesOption(s: SeriesLabelInput): string {
  const base = `${s.patient_id} — ${s.modality} (${s.slice_count} slices)`;
  if (!s.series_description) return base;
  return `${s.patient_id} — ${s.series_description} — ${s.modality} (${s.slice_count} slices)`;
}
