// Fixed HU range the whole volume pipeline normalises against. Chosen to
// cover the full clinical range (air floor, dense-bone/contrast ceiling)
// with headroom, matching the raymarcher shader's HU_MIN/HU_MAX constants.
export const HU_MIN = -1024;
export const HU_MAX = 3071;
export const HU_RANGE = HU_MAX - HU_MIN;

/** Maps an HU value onto [0,1] over the fixed clinical range, clamping outside it. */
export function normalizeHU(hu: number): number {
  return Math.min(1, Math.max(0, (hu - HU_MIN) / HU_RANGE));
}

/** Inverse of normalizeHU; matches the shader's `hu = n * (3071+1024) - 1024`. */
export function denormalizeHU(n: number): number {
  return n * HU_RANGE + HU_MIN;
}

export interface Extent {
  x: number;
  y: number;
  z: number;
}

/**
 * Physical size of the volume per axis (dim * spacing), normalised so the
 * largest axis is 1.0. This is the box the raymarcher intersects — using
 * dims alone (a unit cube) ignores the ~8.5x in-plane-vs-slice-thickness
 * anisotropy and renders the patient crushed along z.
 */
export function physicalExtent(
  dimX: number,
  dimY: number,
  dimZ: number,
  spacingX: number,
  spacingY: number,
  spacingZ: number
): Extent {
  const px = dimX * spacingX;
  const py = dimY * spacingY;
  const pz = dimZ * spacingZ;
  const max = Math.max(px, py, pz);
  if (max <= 0) return { x: 0, y: 0, z: 0 };
  return { x: px / max, y: py / max, z: pz / max };
}
