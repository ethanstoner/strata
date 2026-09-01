export interface Window {
  center: number;
  width: number;
}

export const PRESETS: Record<string, Window> = {
  lung: { center: -600, width: 1500 },
  bone: { center: 300, width: 1500 },
  brain: { center: 40, width: 80 },
  softTissue: { center: 50, width: 400 },
  mediastinum: { center: 50, width: 350 },
};

/**
 * Maps a Hounsfield Unit value through a radiology window to a 0-255 grey
 * level, clamping outside the window's floor/ceiling.
 */
export function huToByte(hu: number, w: Window): number {
  // A non-positive width has no meaningful window; treat it as a hard
  // threshold at the centre instead of dividing by zero (which yields NaN
  // and renders as an undefined/black pixel).
  if (w.width <= 0) {
    return hu < w.center ? 0 : 255;
  }
  const floor = w.center - w.width * 0.5;
  const normalized = (hu - floor) / w.width;
  const clamped = Math.min(1, Math.max(0, normalized));
  return Math.round(clamped * 255);
}
