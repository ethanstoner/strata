import { describe, it, expect } from "vitest";
import {
  buildTransferFunctionLUT,
  huAtLutIndex,
  lutIndexForHU,
  LUT_SIZE,
  TRANSFER_PRESETS,
} from "./transferfunction";
import { HU_MIN, HU_MAX } from "./volumemath";

describe("huAtLutIndex / lutIndexForHU", () => {
  it("are inverses at the LUT endpoints", () => {
    expect(huAtLutIndex(0)).toBeCloseTo(HU_MIN, 9);
    expect(huAtLutIndex(LUT_SIZE - 1)).toBeCloseTo(HU_MAX, 9);
    expect(lutIndexForHU(HU_MIN)).toBeCloseTo(0, 9);
    expect(lutIndexForHU(HU_MAX)).toBeCloseTo(LUT_SIZE - 1, 9);
  });

  it("round-trips an arbitrary index", () => {
    const hu = huAtLutIndex(200);
    expect(lutIndexForHU(hu)).toBeCloseTo(200, 6);
  });
});

describe("buildTransferFunctionLUT", () => {
  it("produces a flat RGBA8 buffer of the requested size", () => {
    const lut = buildTransferFunctionLUT([{ hu: 0, r: 1, g: 1, b: 1, a: 1 }]);
    expect(lut.length).toBe(LUT_SIZE * 4);
    expect(lut).toBeInstanceOf(Uint8Array);
  });

  it("places a control point's exact colour at its corresponding LUT index", () => {
    const idx = 200;
    const hu = huAtLutIndex(idx);
    const lut = buildTransferFunctionLUT([
      { hu: HU_MIN, r: 0, g: 0, b: 0, a: 0 },
      { hu, r: 0.2, g: 0.4, b: 0.6, a: 0.8 },
      { hu: HU_MAX, r: 0.2, g: 0.4, b: 0.6, a: 0.8 },
    ]);
    expect(lut[idx * 4 + 0]).toBe(Math.round(0.2 * 255));
    expect(lut[idx * 4 + 1]).toBe(Math.round(0.4 * 255));
    expect(lut[idx * 4 + 2]).toBe(Math.round(0.6 * 255));
    expect(lut[idx * 4 + 3]).toBe(Math.round(0.8 * 255));
  });

  it("linearly interpolates between two control points", () => {
    const lut = buildTransferFunctionLUT([
      { hu: HU_MIN, r: 0, g: 0, b: 0, a: 0 },
      { hu: HU_MAX, r: 1, g: 1, b: 1, a: 1 },
    ]);
    const mid = Math.round((LUT_SIZE - 1) / 2);
    // Midpoint of a linear ramp from 0 to 255 across the full LUT.
    expect(lut[mid * 4 + 0]).toBeGreaterThan(120);
    expect(lut[mid * 4 + 0]).toBeLessThan(135);
  });

  it("clamps out-of-range control point HU to the nearest edge", () => {
    const lut = buildTransferFunctionLUT([{ hu: HU_MAX + 5000, r: 1, g: 0, b: 0, a: 1 }]);
    // A single control point outside range still fills the whole LUT with
    // its (clamped) colour, since sampleControlPoints treats it as both ends.
    expect(lut[0]).toBe(255);
    expect(lut[3]).toBe(255);
    expect(lut[(LUT_SIZE - 1) * 4]).toBe(255);
  });

  it("bone preset is fully transparent below the ~150 HU soft-tissue cutoff", () => {
    const lut = buildTransferFunctionLUT(TRANSFER_PRESETS.bone);
    const idx = Math.round(lutIndexForHU(0)); // water, well below bone
    expect(lut[idx * 4 + 3]).toBe(0);
  });

  it("bone preset is opaque and near-white at dense bone HU", () => {
    const lut = buildTransferFunctionLUT(TRANSFER_PRESETS.bone);
    const idx = Math.round(lutIndexForHU(1500));
    expect(lut[idx * 4 + 3]).toBeGreaterThan(240);
    expect(lut[idx * 4 + 0]).toBeGreaterThan(240);
  });

  it("soft preset gives semi-transparent warm tones to soft tissue", () => {
    const lut = buildTransferFunctionLUT(TRANSFER_PRESETS.soft);
    const idx = Math.round(lutIndexForHU(40)); // soft tissue
    const alpha = lut[idx * 4 + 3];
    expect(alpha).toBeGreaterThan(0);
    expect(alpha).toBeLessThan(255);
    // warm: red channel should exceed blue for a warm tone
    expect(lut[idx * 4 + 0]).toBeGreaterThan(lut[idx * 4 + 2]);
  });
});
