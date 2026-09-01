import { describe, it, expect } from "vitest";
import { normalizeHU, denormalizeHU, physicalExtent, HU_MIN, HU_MAX } from "./volumemath";

describe("normalizeHU / denormalizeHU", () => {
  it("maps the HU floor to 0 and the HU ceiling to 1", () => {
    expect(normalizeHU(HU_MIN)).toBe(0);
    expect(normalizeHU(HU_MAX)).toBe(1);
  });

  it("clamps outside the fixed range", () => {
    expect(normalizeHU(HU_MIN - 500)).toBe(0);
    expect(normalizeHU(HU_MAX + 500)).toBe(1);
  });

  it("round-trips arbitrary in-range HU values", () => {
    for (const hu of [-1000, -600, -100, 0, 40, 300, 1500, 3000]) {
      const n = normalizeHU(hu);
      expect(denormalizeHU(n)).toBeCloseTo(hu, 9);
    }
  });

  it("denormalize matches the shader's literal formula", () => {
    // hu = normalised * (3071.0 + 1024.0) - 1024.0
    expect(denormalizeHU(0.5)).toBeCloseTo(0.5 * (3071 + 1024) - 1024, 9);
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
