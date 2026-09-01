import { describe, it, expect } from "vitest";
import { formatSeriesOption } from "./seriespicker";

describe("formatSeriesOption", () => {
  it("includes the series description when present", () => {
    expect(
      formatSeriesOption({
        patient_id: "TCGA-17-Z018",
        series_description: "Chest Routine #1",
        modality: "CT",
        slice_count: 60,
      })
    ).toBe("TCGA-17-Z018 — Chest Routine #1 — CT (60 slices)");
  });

  it("falls back to the description-less format when series_description is null", () => {
    expect(
      formatSeriesOption({
        patient_id: "TCGA-17-Z018",
        series_description: null,
        modality: "CT",
        slice_count: 60,
      })
    ).toBe("TCGA-17-Z018 — CT (60 slices)");
  });

  it("falls back when series_description is omitted entirely", () => {
    expect(
      formatSeriesOption({
        patient_id: "TCGA-17-Z018",
        modality: "CT",
        slice_count: 60,
      })
    ).toBe("TCGA-17-Z018 — CT (60 slices)");
  });

  it("falls back when series_description is an empty string, never inventing a placeholder", () => {
    const label = formatSeriesOption({
      patient_id: "TCGA-17-Z018",
      series_description: "",
      modality: "CT",
      slice_count: 60,
    });
    expect(label).toBe("TCGA-17-Z018 — CT (60 slices)");
    expect(label).not.toMatch(/unknown/i);
    expect(label).not.toMatch(/series 1/i);
  });

  it("never emits a placeholder word anywhere in the null-description case", () => {
    const label = formatSeriesOption({
      patient_id: "P1",
      series_description: null,
      modality: "MR",
      slice_count: 1026,
    });
    expect(label).not.toMatch(/unknown/i);
    expect(label).not.toMatch(/n\/a/i);
    expect(label).not.toMatch(/untitled/i);
  });
});
