import { describe, it, expect } from "vitest";
import { huToByte, PRESETS } from "./windowing";

describe("huToByte", () => {
  it("maps the window centre to mid grey (128)", () => {
    expect(huToByte(40, { center: 40, width: 400 })).toBe(128);
  });

  it("clamps values below the floor to 0", () => {
    expect(huToByte(-1000, { center: 40, width: 400 })).toBe(0);
  });

  it("clamps values above the ceiling to 255", () => {
    expect(huToByte(2000, { center: 40, width: 400 })).toBe(255);
  });

  it("lung preset maps air well below mid grey and water well above it", () => {
    // Preset is center -600 / width 1500 -> range [-1350, 150]. Air (-1000)
    // sits in the low third of that range; water (0) sits in the top decile.
    const air = huToByte(-1000, PRESETS.lung);
    const water = huToByte(0, PRESETS.lung);
    expect(air).toBeLessThan(90);
    expect(water).toBeGreaterThan(200);
  });

  it("does not produce NaN for a zero width window", () => {
    const result = huToByte(40, { center: 40, width: 0 });
    expect(Number.isNaN(result)).toBe(false);
  });

  it("does not produce NaN for a negative width window", () => {
    const result = huToByte(40, { center: 40, width: -50 });
    expect(Number.isNaN(result)).toBe(false);
  });
});
