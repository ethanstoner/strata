import { describe, it, expect } from "vitest";
import {
  normalizeHU,
  denormalizeHU,
  physicalExtent,
  requiredSteps,
  levelZeroBytes,
  levelDims,
  levelBytes,
  computeLevelOptions,
  chooseDefaultLevel,
  HU_MIN,
  HU_MAX,
  MAX_RAYMARCH_STEPS,
  MAX_VOLUME_BYTES,
  DEFAULT_HU_RANGE,
  DEFAULT_LEVEL_BUDGET_BYTES,
  type HuRange,
} from "./volumemath";

describe("normalizeHU / denormalizeHU", () => {
  it("maps the HU floor to 0 and the HU ceiling to 1 over the default fixed range", () => {
    expect(normalizeHU(HU_MIN)).toBe(0);
    expect(normalizeHU(HU_MAX)).toBe(1);
  });

  it("clamps outside the fixed range", () => {
    expect(normalizeHU(HU_MIN - 500)).toBe(0);
    expect(normalizeHU(HU_MAX + 500)).toBe(1);
  });

  it("round-trips arbitrary in-range HU values over the default fixed range", () => {
    for (const hu of [-1000, -600, -100, 0, 40, 300, 1500, 3000]) {
      const n = normalizeHU(hu);
      expect(denormalizeHU(n)).toBeCloseTo(hu, 9);
    }
  });

  it("denormalize matches the shader's literal formula for the default range", () => {
    // hu = normalised * (3071.0 + 1024.0) - 1024.0
    expect(denormalizeHU(0.5)).toBeCloseTo(0.5 * (3071 + 1024) - 1024, 9);
  });

  it("defaults to DEFAULT_HU_RANGE when no range is passed", () => {
    expect(normalizeHU(500)).toBeCloseTo(normalizeHU(500, DEFAULT_HU_RANGE), 12);
  });

  it("normalises against a real per-volume range instead of the fixed default", () => {
    // Measured real study: hu_min is -2048 (CT out-of-reconstruction-circle
    // fill value), well below the fixed -1024 floor. A fixed-range
    // normalize would clamp this to 0 and lose it; a range-aware one must
    // not.
    const range: HuRange = { min: -2048, max: 3071 };
    expect(normalizeHU(-2048, range)).toBe(0);
    expect(normalizeHU(3071, range)).toBe(1);
    // -1500 HU is below the fixed range's floor (-1024), so the fixed
    // default clamps it to 0; the real per-volume range must not.
    expect(normalizeHU(-1500, DEFAULT_HU_RANGE)).toBe(0);
    expect(normalizeHU(-1500, range)).toBeGreaterThan(0);
  });

  it("round-trips arbitrary in-range HU values over a custom range", () => {
    const range: HuRange = { min: -2048, max: 2000 };
    for (const hu of [-2048, -1500, -500, 0, 300, 1999, 2000]) {
      const n = normalizeHU(hu, range);
      expect(denormalizeHU(n, range)).toBeCloseTo(hu, 9);
    }
  });

  it("a control-point-style HU value lands at the same normalised position it was authored at, for any range", () => {
    // Stand-in for the transfer function requirement: a bone control point
    // authored at 300 HU must map to a consistent, invertible texel
    // position regardless of which volume's range is in effect.
    for (const range of [DEFAULT_HU_RANGE, { min: -2048, max: 3071 }, { min: -1024, max: 1024 }]) {
      const n = normalizeHU(300, range);
      expect(denormalizeHU(n, range)).toBeCloseTo(300, 6);
    }
  });

  it("guards the degenerate hu_max === hu_min case instead of producing NaN", () => {
    const range: HuRange = { min: 40, max: 40 };
    expect(normalizeHU(40, range)).not.toBeNaN();
    expect(normalizeHU(999, range)).not.toBeNaN();
    expect(denormalizeHU(0.5, range)).not.toBeNaN();
    expect(normalizeHU(40, range)).toBe(0);
    expect(denormalizeHU(0.5, range)).toBe(40);
  });
});

describe("physicalExtent", () => {
  it("produces a unit cube for isotropic spacing and dims", () => {
    const e = physicalExtent(100, 100, 100, 1, 1, 1);
    expect(e.x).toBeCloseTo(1, 9);
    expect(e.y).toBeCloseTo(1, 9);
    expect(e.z).toBeCloseTo(1, 9);
  });

  it("compresses the short axis for anisotropic spacing (sample dataset shape)", () => {
    // 512x512x60 at 0.59375mm in-plane / 5.0mm between slices.
    const e = physicalExtent(512, 512, 60, 0.59375, 0.59375, 5.0);
    const physX = 512 * 0.59375; // 304.0
    const physZ = 60 * 5.0; // 300.0
    expect(e.x).toBeCloseTo(1, 9);
    expect(e.y).toBeCloseTo(1, 9);
    expect(e.z).toBeCloseTo(physZ / physX, 9);
    // Sanity: physical z is nearly as large as x/y here, NOT 60/512 (~0.117)
    // as a naive unit-cube-from-dims computation would give.
    expect(e.z).toBeGreaterThan(0.9);
    expect(60 / 512).toBeLessThan(0.15);
  });

  it("scales correctly for a halved (level 1) volume", () => {
    // Level 1: dims halved, spacing doubled vs level 0 -> same physical size.
    const level0 = physicalExtent(512, 512, 60, 0.59375, 0.59375, 5.0);
    const level1 = physicalExtent(256, 256, 30, 1.1875, 1.1875, 10.0);
    expect(level1.x).toBeCloseTo(level0.x, 9);
    expect(level1.y).toBeCloseTo(level0.y, 9);
    expect(level1.z).toBeCloseTo(level0.z, 9);
  });

  it("returns zero extent for a degenerate zero-size volume", () => {
    const e = physicalExtent(0, 0, 0, 1, 1, 1);
    expect(e).toEqual({ x: 0, y: 0, z: 0 });
  });
});

describe("requiredSteps", () => {
  it("needs more than 512 steps for a 256x256x513 volume at oversample 1.0 (the measured aliasing case)", () => {
    const steps = requiredSteps({ x: 256, y: 256, z: 513 }, 1.0);
    expect(steps).toBeGreaterThan(512);
    // Sanity on the actual diagonal: sqrt(256^2+256^2+513^2) ~= 627.9
    expect(steps).toBe(Math.ceil(Math.sqrt(256 * 256 + 256 * 256 + 513 * 513)));
  });

  it("needs far fewer steps for a small 64^3 volume", () => {
    const small = requiredSteps({ x: 64, y: 64, z: 64 }, 1.0);
    const deep = requiredSteps({ x: 256, y: 256, z: 513 }, 1.0);
    expect(small).toBeLessThan(200);
    expect(small).toBeLessThan(deep);
  });

  it("scales linearly with the oversample factor", () => {
    const dims = { x: 300, y: 300, z: 300 }; // diagonal is exact-ish, avoids rounding noise
    const at1 = requiredSteps(dims, 1.0);
    const at2 = requiredSteps(dims, 2.0);
    expect(at2).toBeCloseTo(at1 * 2, -1); // within ~10 steps, i.e. rounding-only slack
  });

  it("never exceeds MAX_RAYMARCH_STEPS even for an enormous volume", () => {
    const steps = requiredSteps({ x: 4096, y: 4096, z: 4096 }, 4.0);
    expect(steps).toBe(MAX_RAYMARCH_STEPS);
  });

  it("never returns fewer than 1 step for a degenerate zero-size volume", () => {
    const steps = requiredSteps({ x: 0, y: 0, z: 0 }, 1.0);
    expect(steps).toBeGreaterThanOrEqual(1);
  });
});

describe("levelZeroBytes / MAX_VOLUME_BYTES", () => {
  it("flags a 512x512x1026 study (the measured 1026-slice study) as over the 512MB server limit", () => {
    const bytes = levelZeroBytes(512, 512, 1026);
    expect(bytes).toBeGreaterThan(MAX_VOLUME_BYTES);
  });

  it("flags a 512x512x60 study as comfortably under the limit", () => {
    const bytes = levelZeroBytes(512, 512, 60);
    expect(bytes).toBeLessThan(MAX_VOLUME_BYTES);
  });

  it("matches the raw i16-per-voxel byte count exactly", () => {
    expect(levelZeroBytes(10, 20, 30)).toBe(10 * 20 * 30 * 2);
  });
});

describe("levelDims / levelBytes / computeLevelOptions", () => {
  // The measured 1026-slice study: 512x512x1026.
  const DIM_X = 512;
  const DIM_Y = 512;
  const DIM_Z = 1026;

  it("halves each axis (ceil) per level, matching the server's output_dims", () => {
    expect(levelDims(DIM_X, DIM_Y, DIM_Z, 0)).toEqual({ x: 512, y: 512, z: 1026 });
    expect(levelDims(DIM_X, DIM_Y, DIM_Z, 1)).toEqual({ x: 256, y: 256, z: 513 });
    expect(levelDims(DIM_X, DIM_Y, DIM_Z, 2)).toEqual({ x: 128, y: 128, z: 257 });
    expect(levelDims(DIM_X, DIM_Y, DIM_Z, 3)).toEqual({ x: 64, y: 64, z: 129 });
  });

  it("rounds an odd dimension up rather than truncating (never drops a slice)", () => {
    // 1026 / 4 = 256.5 -> must ceil to 257, not floor to 256.
    expect(levelDims(DIM_X, DIM_Y, DIM_Z, 2).z).toBe(257);
  });

  it("levelBytes matches dims.x * dims.y * dims.z * 2 for every level", () => {
    for (let level = 0; level <= 3; level++) {
      const dims = levelDims(DIM_X, DIM_Y, DIM_Z, level);
      expect(levelBytes(DIM_X, DIM_Y, DIM_Z, level)).toBe(dims.x * dims.y * dims.z * 2);
    }
  });

  it("levelZeroBytes agrees with levelBytes at level 0", () => {
    expect(levelZeroBytes(DIM_X, DIM_Y, DIM_Z)).toBe(levelBytes(DIM_X, DIM_Y, DIM_Z, 0));
  });

  it("flags level 0 unavailable and level 1 available for the 512x512x1026 study (the measured case)", () => {
    const options = computeLevelOptions(DIM_X, DIM_Y, DIM_Z);
    const byLevel = new Map(options.map((o) => [o.level, o]));
    expect(byLevel.get(0)!.bytes).toBeGreaterThan(MAX_VOLUME_BYTES);
    expect(byLevel.get(0)!.available).toBe(false);
    expect(byLevel.get(1)!.bytes).toBeLessThan(MAX_VOLUME_BYTES);
    expect(byLevel.get(1)!.available).toBe(true);
  });

  it("returns exactly levels 0-3, each with the dims/bytes pairing", () => {
    const options = computeLevelOptions(DIM_X, DIM_Y, DIM_Z);
    expect(options.map((o) => o.level)).toEqual([0, 1, 2, 3]);
    for (const opt of options) {
      expect(opt.bytes).toBe(opt.dims.x * opt.dims.y * opt.dims.z * 2);
    }
  });

  it("marks every level available for a small study well under the byte limit", () => {
    const options = computeLevelOptions(512, 512, 60);
    expect(options.every((o) => o.available)).toBe(true);
  });
});

describe("chooseDefaultLevel", () => {
  it("picks level 1 for the 512x512x1026 study at the default ~96MB budget", () => {
    // level 0 is ~513MB (over the 512MB server limit, let alone the budget);
    // level 1 is ~64MB, comfortably under DEFAULT_LEVEL_BUDGET_BYTES (96MB).
    expect(chooseDefaultLevel(512, 512, 1026)).toBe(1);
  });

  it("picks the most detailed level that still fits a smaller budget", () => {
    const level1Bytes = levelBytes(512, 512, 1026, 1);
    // A budget just under level 1's size should push the choice to level 2.
    expect(chooseDefaultLevel(512, 512, 1026, level1Bytes - 1)).toBe(2);
  });

  it("picks level 0 for a small study whose full resolution already fits the budget", () => {
    expect(chooseDefaultLevel(512, 512, 60, DEFAULT_LEVEL_BUDGET_BYTES)).toBe(0);
  });

  it("falls back to the lightest level (3) when nothing fits the budget", () => {
    expect(chooseDefaultLevel(512, 512, 1026, 0)).toBe(3);
  });
});
