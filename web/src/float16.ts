// Minimal float32 -> IEEE754 half-float (binary16) bit-pattern encoder.
// JS has no half-float type; gl.HALF_FLOAT texture uploads require the raw
// 16-bit pattern in a Uint16Array. This is intentionally NOT general-purpose:
// it only needs to handle finite values in [0,1], since that's the domain of
// normalizeHU() output, so subnormal/negative/NaN/Infinity paths are elided.
const floatView = new Float32Array(1);
const uint32View = new Uint32Array(floatView.buffer);

export function floatToHalf(value: number): number {
  if (value <= 0) return 0;
  if (value >= 1) return 0x3c00; // half-precision 1.0, exact
  floatView[0] = value;
  const bits = uint32View[0];
  const exponent = ((bits >> 23) & 0xff) - 127 + 15;
  let mantissa = bits & 0x7fffff;
  // Round-to-nearest on the 13 mantissa bits half-float drops relative to
  // float32, rather than truncating (which would bias every value down).
  const roundBit = mantissa & 0x1000;
  mantissa = mantissa >>> 13;
  if (roundBit) mantissa += 1;
  if (mantissa === 0x400) {
    // Mantissa rounded up to the next power of two; bump the exponent.
    return ((exponent + 1) << 10) | 0;
  }
  return (exponent << 10) | mantissa;
}
