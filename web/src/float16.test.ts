import { describe, it, expect } from "vitest";
import { floatToHalf } from "./float16";

describe("floatToHalf", () => {
  it("encodes 0 as 0", () => {
    expect(floatToHalf(0)).toBe(0);
  });

  it("encodes 1.0 as the exact half-float bit pattern 0x3C00", () => {
    expect(floatToHalf(1)).toBe(0x3c00);
  });

  it("encodes 0.5 as the exact half-float bit pattern 0x3800", () => {
    expect(floatToHalf(0.5)).toBe(0x3800);
  });

  it("clamps values above 1 to the encoding for 1.0", () => {
    expect(floatToHalf(1.5)).toBe(0x3c00);
  });

  it("clamps negative values to 0", () => {
    expect(floatToHalf(-0.2)).toBe(0);
  });

  it("is monotonically non-decreasing across [0,1]", () => {
    let prev = -1;
    for (let i = 0; i <= 100; i++) {
      const bits = floatToHalf(i / 100);
      expect(bits).toBeGreaterThanOrEqual(prev);
      prev = bits;
    }
  });
});
