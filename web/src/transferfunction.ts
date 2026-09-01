import { HU_MIN, HU_MAX, HU_RANGE, normalizeHU, denormalizeHU } from "./volumemath";

/** A HU->RGBA control point. r/g/b/a are all in [0,1]. */
export interface ControlPoint {
  hu: number;
  r: number;
  g: number;
  b: number;
  a: number;
}

export const LUT_SIZE = 256;

/** HU value that LUT texel `index` (of `size`) represents. Inverse of lutIndexForHU. */
export function huAtLutIndex(index: number, size: number = LUT_SIZE): number {
  return denormalizeHU(index / (size - 1));
}

/** Fractional LUT index (of `size`) a given HU value maps to. */
export function lutIndexForHU(hu: number, size: number = LUT_SIZE): number {
  return normalizeHU(hu) * (size - 1);
}

function clamp01(v: number): number {
  return Math.min(1, Math.max(0, v));
}

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

function sampleControlPoints(sorted: ControlPoint[], hu: number): [number, number, number, number] {
  if (sorted.length === 0) return [0, 0, 0, 0];
  const first = sorted[0];
  if (hu <= first.hu) return [first.r, first.g, first.b, first.a];
  const last = sorted[sorted.length - 1];
  if (hu >= last.hu) return [last.r, last.g, last.b, last.a];
  for (let i = 0; i < sorted.length - 1; i++) {
    const p0 = sorted[i];
    const p1 = sorted[i + 1];
    if (hu >= p0.hu && hu <= p1.hu) {
      const t = p1.hu === p0.hu ? 0 : (hu - p0.hu) / (p1.hu - p0.hu);
      return [lerp(p0.r, p1.r, t), lerp(p0.g, p1.g, t), lerp(p0.b, p1.b, t), lerp(p0.a, p1.a, t)];
    }
  }
  return [last.r, last.g, last.b, last.a];
}

/**
 * Builds a `size`x1 RGBA8 LUT (flat Uint8Array, 4 bytes/texel) from HU->RGBA
 * control points, piecewise-linearly interpolated in HU space. Texel index i
 * corresponds to HU = huAtLutIndex(i, size) — the same normalised-HU domain
 * the volume texture's R16F values live in, so the shader can sample this
 * LUT directly with the volume's raw (normalised) texture read as the x
 * coordinate, no per-sample HU conversion needed for the transfer function.
 */
export function buildTransferFunctionLUT(points: ControlPoint[], size: number = LUT_SIZE): Uint8Array {
  const sorted = [...points].sort((a, b) => a.hu - b.hu);
  const out = new Uint8Array(size * 4);
  for (let i = 0; i < size; i++) {
    const hu = huAtLutIndex(i, size);
    const [r, g, b, a] = sampleControlPoints(sorted, hu);
    out[i * 4 + 0] = Math.round(clamp01(r) * 255);
    out[i * 4 + 1] = Math.round(clamp01(g) * 255);
    out[i * 4 + 2] = Math.round(clamp01(b) * 255);
    out[i * 4 + 3] = Math.round(clamp01(a) * 255);
  }
  return out;
}

/**
 * Preset control points, editable HU->RGBA. Two presets shipped per spec:
 * `bone` hides everything below ~150 HU and renders bone near-white/opaque;
 * `soft` gives skin/muscle warm semi-transparent tones with bone still opaque.
 */
export const TRANSFER_PRESETS: Record<string, ControlPoint[]> = {
  bone: [
    { hu: HU_MIN, r: 0, g: 0, b: 0, a: 0 },
    { hu: 140, r: 0, g: 0, b: 0, a: 0 },
    { hu: 150, r: 0.85, g: 0.82, b: 0.75, a: 0.55 },
    { hu: 400, r: 0.95, g: 0.94, b: 0.9, a: 0.9 },
    { hu: 1200, r: 1, g: 1, b: 1, a: 1 },
    { hu: HU_MAX, r: 1, g: 1, b: 1, a: 1 },
  ],
  soft: [
    { hu: HU_MIN, r: 0, g: 0, b: 0, a: 0 },
    { hu: -300, r: 0, g: 0, b: 0, a: 0 },
    { hu: -100, r: 0.9, g: 0.7, b: 0.55, a: 0.05 },
    { hu: 40, r: 0.85, g: 0.55, b: 0.45, a: 0.15 },
    { hu: 150, r: 0.9, g: 0.75, b: 0.6, a: 0.35 },
    { hu: 400, r: 0.95, g: 0.94, b: 0.9, a: 0.85 },
    { hu: 1200, r: 1, g: 1, b: 1, a: 1 },
    { hu: HU_MAX, r: 1, g: 1, b: 1, a: 1 },
  ],
};

export { HU_RANGE };
