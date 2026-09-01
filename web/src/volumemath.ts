// Fixed HU range used when the server hasn't reported a per-volume range
// (e.g. no volume loaded yet). Chosen to cover the full clinical range (air
// floor, dense-bone/contrast ceiling) with headroom. Real studies report
// their own hu_min/hu_max (see api.ts's X-Strata-HU-Min/Max headers) which
// can and do fall outside this window — e.g. -2048 is a common CT
// out-of-reconstruction-circle fill value — so this is a fallback, not an
// assumption baked into the math.
export const HU_MIN = -1024;
export const HU_MAX = 3071;
export const HU_RANGE = HU_MAX - HU_MIN;

export interface HuRange {
  min: number;
  max: number;
}

export const DEFAULT_HU_RANGE: HuRange = { min: HU_MIN, max: HU_MAX };

/**
 * Maps an HU value onto [0,1] over `range`, clamping outside it. Defaults to
 * the fixed clinical range for callers that don't have a per-volume range
 * (or haven't loaded a volume yet).
 *
 * Guards `range.max === range.min` (a constant volume, or an unset range) —
 * without this, the division below produces NaN, which the 3D texture would
 * happily upload and render as an undefined/garbage voxel rather than
 * failing loudly.
 */
export function normalizeHU(hu: number, range: HuRange = DEFAULT_HU_RANGE): number {
  const span = range.max - range.min;
  if (span <= 0) return 0;
  return Math.min(1, Math.max(0, (hu - range.min) / span));
}

/** Inverse of normalizeHU over the same `range`. */
export function denormalizeHU(n: number, range: HuRange = DEFAULT_HU_RANGE): number {
  const span = range.max - range.min;
  if (span <= 0) return range.min;
  return n * span + range.min;
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

// Hard ceiling on the raymarch step count. Independent of how large the
// volume's diagonal gets (a deep, high-res study can demand thousands of
// steps per pixel), this keeps a single frame's worst-case cost bounded so a
// low-end GPU doesn't hang the tab; must match the shader's MAX_STEPS trip
// count in volumeview.ts.
export const MAX_RAYMARCH_STEPS = 2048;

/**
 * Minimum raymarch step count for ~one sample per voxel along the worst-case
 * ray (the box diagonal, in voxel units — the longest line segment that can
 * cross the volume), times an oversampling factor. `oversample = 1.0` is the
 * Nyquist floor; a ray marching at exactly one step per voxel still aliases
 * in practice because samples rarely land on voxel centres, so 1.5-2.0 is
 * recommended for a visibly clean render. Capped at MAX_RAYMARCH_STEPS.
 */
export function requiredSteps(dims: Extent, oversample: number): number {
  const diagonal = Math.sqrt(dims.x * dims.x + dims.y * dims.y + dims.z * dims.z);
  const steps = Math.ceil(diagonal * oversample);
  return Math.min(MAX_RAYMARCH_STEPS, Math.max(1, steps));
}

// Mirrors the server's strata-server/src/volume.rs MAX_OUTPUT_BYTES guard —
// the server rejects a volume request whose response would exceed this, so
// the UI predicts that rejection instead of offering a button that 400s.
export const MAX_VOLUME_BYTES = 512 * 1024 * 1024;

// Levels 0-3 are supported; mirrors the server's volume::MAX_LEVEL.
export const MAX_PYRAMID_LEVEL = 3;
export const PYRAMID_LEVELS = [0, 1, 2, 3] as const;

/** Dims (x, y, z) of a series' volume at pyramid `level`, matching the server's output_dims: ceil(dim / 2^level) per axis. */
export function levelDims(dimX: number, dimY: number, dimZ: number, level: number): Extent {
  const factor = 2 ** level;
  return {
    x: Math.ceil(dimX / factor),
    y: Math.ceil(dimY / factor),
    z: Math.ceil(dimZ / factor),
  };
}

/** Projected byte size of a volume response at `level`: raw i16 voxels, 2 bytes each. */
export function levelBytes(dimX: number, dimY: number, dimZ: number, level: number): number {
  const d = levelDims(dimX, dimY, dimZ, level);
  return d.x * d.y * d.z * 2;
}

/** Projected byte size of a full-resolution (level 0) volume response: raw i16 voxels, 2 bytes each. */
export function levelZeroBytes(dimX: number, dimY: number, sliceCount: number): number {
  return levelBytes(dimX, dimY, sliceCount, 0);
}

export interface LevelOption {
  level: number;
  dims: Extent;
  bytes: number;
  /** Whether the server would accept a request for this level (bytes within MAX_VOLUME_BYTES). */
  available: boolean;
}

/** Dims/size/availability for every supported pyramid level (0-3) of a series. */
export function computeLevelOptions(dimX: number, dimY: number, dimZ: number): LevelOption[] {
  return PYRAMID_LEVELS.map((level) => {
    const dims = levelDims(dimX, dimY, dimZ, level);
    const bytes = dims.x * dims.y * dims.z * 2;
    return { level, dims, bytes, available: bytes <= MAX_VOLUME_BYTES };
  });
}

// Default pyramid-level budget: big enough to look detailed on a typical
// laptop GPU, small enough that a huge study (e.g. 512x512x1026) doesn't
// blow past it by default and hang a modest machine.
export const DEFAULT_LEVEL_BUDGET_BYTES = 96 * 1024 * 1024;

/** Picks the most-detailed (lowest-numbered) level whose byte size fits `budgetBytes`, falling back to the smallest/lightest level if none do. */
export function chooseDefaultLevel(
  dimX: number,
  dimY: number,
  dimZ: number,
  budgetBytes: number = DEFAULT_LEVEL_BUDGET_BYTES
): number {
  const options = computeLevelOptions(dimX, dimY, dimZ);
  for (const opt of options) {
    if (opt.bytes <= budgetBytes) return opt.level;
  }
  return options[options.length - 1].level;
}
