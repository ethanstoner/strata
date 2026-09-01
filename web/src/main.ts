import { PRESETS, type Window } from "./windowing";
import { fetchSeriesList, fetchSeriesDetail, fetchSlice, fetchVolume } from "./api";
import type { SeriesSummary, SeriesDetail, SliceData, VolumeData } from "./api";
import { SliceView } from "./sliceview";
import { VolumeView } from "./volumeview";
import { buildTransferFunctionLUT, TRANSFER_PRESETS } from "./transferfunction";
import {
  DEFAULT_HU_RANGE,
  requiredSteps,
  MAX_RAYMARCH_STEPS,
  levelZeroBytes,
  MAX_VOLUME_BYTES,
  type HuRange,
} from "./volumemath";

// Extra samples per voxel along the worst-case ray, beyond the bare Nyquist
// floor (oversample=1.0). 1.0 still visibly aliases in practice since
// samples rarely land on voxel centres; 1.5 cleans that up without doubling
// per-frame cost the way 2.0 would.
const STEP_OVERSAMPLE = 1.5;

type ViewMode = "slices" | "volume";

const canvas = document.querySelector<HTMLCanvasElement>("#gl-canvas")!;
const view = new SliceView(canvas);

const volumeCanvas = document.querySelector<HTMLCanvasElement>("#gl-canvas-volume")!;
const volumeView = new VolumeView(volumeCanvas);

const seriesSelect = document.querySelector<HTMLSelectElement>("#series-select")!;
const emptyState = document.querySelector<HTMLDivElement>("#empty-state")!;
const uncalibratedNotice = document.querySelector<HTMLDivElement>("#uncalibrated-notice")!;
const sliceLabel = document.querySelector<HTMLSpanElement>("#slice-label")!;
const windowLabel = document.querySelector<HTMLSpanElement>("#window-label")!;
const infoPatient = document.querySelector<HTMLElement>("#info-patient")!;
const infoModality = document.querySelector<HTMLElement>("#info-modality")!;
const infoDims = document.querySelector<HTMLElement>("#info-dims")!;
const infoSlices = document.querySelector<HTMLElement>("#info-slices")!;
const presetsContainer = document.querySelector<HTMLDivElement>("#presets")!;

const modeSlicesBtn = document.querySelector<HTMLButtonElement>("#mode-slices")!;
const modeVolumeBtn = document.querySelector<HTMLButtonElement>("#mode-volume")!;
const sliceHint = document.querySelector<HTMLElement>("#slice-hint")!;
const volumeHint = document.querySelector<HTMLElement>("#volume-hint")!;
const volumeSection = document.querySelector<HTMLDivElement>("#volume-section")!;
const volumeOverlay = document.querySelector<HTMLDivElement>("#volume-overlay")!;
const volumeUncalibratedNotice = document.querySelector<HTMLDivElement>(
  "#volume-uncalibrated-notice"
)!;
const volumeLoading = document.querySelector<HTMLDivElement>("#volume-loading")!;
const volumeInfoLabel = document.querySelector<HTMLSpanElement>("#volume-info-label")!;
const windowPresetsSection = document.querySelector<HTMLDivElement>("#window-presets-section")!;
const qualitySlider = document.querySelector<HTMLInputElement>("#quality-slider")!;
const qualityLabel = document.querySelector<HTMLSpanElement>("#quality-label")!;
const opacitySlider = document.querySelector<HTMLInputElement>("#opacity-slider")!;
const opacityLabel = document.querySelector<HTMLSpanElement>("#opacity-label")!;
const thresholdSlider = document.querySelector<HTMLInputElement>("#threshold-slider")!;
const thresholdLabel = document.querySelector<HTMLSpanElement>("#threshold-label")!;
const tfPresetsContainer = document.querySelector<HTMLDivElement>("#tf-presets")!;
const mipToggle = document.querySelector<HTMLInputElement>("#mip-toggle")!;
const loadFullDetailBtn = document.querySelector<HTMLButtonElement>("#load-full-detail")!;

let currentDetail: SeriesDetail | null = null;
let currentOrdinal = 0;
let currentWindow: Window = { ...PRESETS.softTissue };
let sliceCache = new Map<number, SliceData>();
let inFlight = new Map<number, Promise<SliceData>>();
const PREFETCH_RADIUS = 3;

let mode: ViewMode = "slices";
let loadedVolume: { seriesUid: string; level: number } | null = null;
let volumeInFlight: Promise<VolumeData> | null = null;
let currentTfPreset = "bone";
// The actual HU range of the currently loaded volume (server-reported
// hu_min/hu_max). Drives both the transfer function's HU->texel mapping and
// the threshold slider's bounds; falls back to the fixed clinical range
// before any volume has loaded.
let currentVolumeRange: HuRange = DEFAULT_HU_RANGE;

function loadSlice(seriesUid: string, ordinal: number): Promise<SliceData> {
  const cached = sliceCache.get(ordinal);
  if (cached) return Promise.resolve(cached);
  const pending = inFlight.get(ordinal);
  if (pending) return pending;
  const p = fetchSlice(seriesUid, ordinal).then((data) => {
    sliceCache.set(ordinal, data);
    inFlight.delete(ordinal);
    return data;
  });
  inFlight.set(ordinal, p);
  return p;
}

function prefetchNeighbors(seriesUid: string, ordinal: number, total: number): void {
  for (let d = 1; d <= PREFETCH_RADIUS; d++) {
    for (const n of [ordinal - d, ordinal + d]) {
      if (n >= 0 && n < total && !sliceCache.has(n) && !inFlight.has(n)) {
        loadSlice(seriesUid, n).catch(() => {
          /* prefetch failures are silent; the slice will be re-fetched on demand */
        });
      }
    }
  }
}

function updateWindowLabel(): void {
  windowLabel.textContent = `C ${Math.round(currentWindow.center)}  W ${Math.round(currentWindow.width)}`;
}

function updateSliceLabel(): void {
  if (!currentDetail) return;
  const depth = currentDetail.depths[currentOrdinal];
  const depthStr = depth !== undefined && depth !== null ? `${depth.toFixed(1)} mm` : "—";
  sliceLabel.textContent = `slice ${currentOrdinal + 1} / ${currentDetail.slice_count}  (${depthStr})`;
}

function highlightActivePreset(name: string | null): void {
  presetsContainer.querySelectorAll("button.preset").forEach((btn) => {
    btn.classList.toggle("active", btn.getAttribute("data-preset") === name);
  });
}

function highlightActiveTfPreset(name: string | null): void {
  tfPresetsContainer.querySelectorAll("button.preset").forEach((btn) => {
    btn.classList.toggle("active", btn.getAttribute("data-tf-preset") === name);
  });
}

async function showSlice(ordinal: number): Promise<void> {
  if (!currentDetail) return;
  currentOrdinal = ordinal;
  const data = await loadSlice(currentDetail.series_uid, ordinal);
  view.uploadSlice(data.pixels, data.rows, data.cols);
  view.setWindow(currentWindow);
  view.render();
  updateSliceLabel();
  prefetchNeighbors(currentDetail.series_uid, ordinal, currentDetail.slice_count);
}

function buildPresetButtons(): void {
  presetsContainer.innerHTML = "";
  for (const name of Object.keys(PRESETS)) {
    const btn = document.createElement("button");
    btn.className = "preset";
    btn.type = "button";
    btn.textContent = name;
    btn.setAttribute("data-preset", name);
    btn.addEventListener("click", () => {
      currentWindow = { ...PRESETS[name] };
      highlightActivePreset(name);
      updateWindowLabel();
      view.setWindow(currentWindow);
      view.render();
      volumeView.setWindow(currentWindow);
      if (mode === "volume") volumeView.render();
    });
    presetsContainer.appendChild(btn);
  }
}

function buildTfPresetButtons(): void {
  tfPresetsContainer.innerHTML = "";
  for (const name of Object.keys(TRANSFER_PRESETS)) {
    const btn = document.createElement("button");
    btn.className = "preset";
    btn.type = "button";
    btn.textContent = name;
    btn.setAttribute("data-tf-preset", name);
    btn.addEventListener("click", () => {
      currentTfPreset = name;
      applyTransferFunctionPreset();
      highlightActiveTfPreset(name);
    });
    tfPresetsContainer.appendChild(btn);
  }
}

function applyTransferFunctionPreset(): void {
  const points = TRANSFER_PRESETS[currentTfPreset];
  // Must build the LUT over the same HU range the volume texture was
  // normalised with, or a control point's HU (e.g. bone at 300) lands on
  // the wrong texel relative to what the shader samples for that voxel.
  volumeView.setTransferFunction(buildTransferFunctionLUT(points, undefined, currentVolumeRange));
  if (mode === "volume") volumeView.render();
}

async function loadVolumeIfNeeded(seriesUid: string, level = 1): Promise<void> {
  if (loadedVolume && loadedVolume.seriesUid === seriesUid && loadedVolume.level === level) {
    return;
  }
  volumeLoading.style.display = "block";
  try {
    const data = volumeInFlight ?? fetchVolume(seriesUid, level);
    volumeInFlight = data;
    const volume = await data;
    volumeInFlight = null;

    // hu_min == hu_max would mean a constant volume; guard so the range
    // math (division in normalizeHU) can't degrade to a NaN spread. Falls
    // back to the fixed clinical range rather than a zero-width one.
    currentVolumeRange =
      volume.huMax > volume.huMin ? { min: volume.huMin, max: volume.huMax } : DEFAULT_HU_RANGE;

    volumeView.uploadVolume(
      volume.voxels,
      volume.dimX,
      volume.dimY,
      volume.dimZ,
      volume.spacingX,
      volume.spacingY,
      volume.spacingZ,
      currentVolumeRange
    );
    loadedVolume = { seriesUid, level: volume.level };
    volumeUncalibratedNotice.style.display = volume.huCalibrated ? "none" : "block";
    volumeInfoLabel.textContent = `level ${volume.level}  ·  ${volume.dimX}x${volume.dimY}x${volume.dimZ}`;

    // The transfer function's HU->texel mapping depends on the range just
    // set above, so it has to be rebuilt for this volume.
    applyTransferFunctionPreset();

    // Threshold slider must span what this volume can actually contain —
    // a hardcoded [-1024, 3071] both misrepresents the data (real hu_min
    // can be well below -1024, e.g. CT's -2048 fill value) and can't reach
    // parts of a narrower range. Clamp rather than reset so a user's chosen
    // cutoff survives switching pyramid levels of the same series.
    thresholdSlider.min = String(currentVolumeRange.min);
    thresholdSlider.max = String(currentVolumeRange.max);
    const clampedThreshold = Math.min(
      currentVolumeRange.max,
      Math.max(currentVolumeRange.min, Number(thresholdSlider.value))
    );
    thresholdSlider.value = String(clampedThreshold);
    thresholdLabel.textContent = `${clampedThreshold} HU`;
    volumeView.setThresholdHU(clampedThreshold);

    // Default step count from the volume's actual depth: a fixed 256 is
    // fewer than one sample per voxel along the box diagonal for a deep
    // study (e.g. 256x256x513), which aliases as visible banding. Derive it
    // instead, but keep the slider usable for turning quality down on weak
    // hardware.
    const defaultSteps = requiredSteps(
      { x: volume.dimX, y: volume.dimY, z: volume.dimZ },
      STEP_OVERSAMPLE
    );
    qualitySlider.min = "32";
    qualitySlider.max = String(MAX_RAYMARCH_STEPS);
    qualitySlider.value = String(defaultSteps);
    qualityLabel.textContent = String(defaultSteps);
    volumeView.setSteps(defaultSteps);

    volumeView.render();
  } finally {
    volumeLoading.style.display = "none";
  }
}

// Window centre/width only affect the render in slice mode or in volume
// mode's MIP path (see volumeview.ts's FRAG_SRC: uWindowCenter/Width are
// only read under uMipMode==1). Showing the presets in normal volume mode
// offers a control that silently does nothing when clicked.
function updateWindowPresetsVisibility(): void {
  const show = mode === "slices" || mipToggle.checked;
  windowPresetsSection.style.display = show ? "block" : "none";
}

function updateFullDetailButton(): void {
  if (!currentDetail) {
    loadFullDetailBtn.disabled = true;
    return;
  }
  const bytes = levelZeroBytes(currentDetail.cols, currentDetail.rows, currentDetail.slice_count);
  if (bytes > MAX_VOLUME_BYTES) {
    loadFullDetailBtn.disabled = true;
    const mb = Math.round(bytes / (1024 * 1024));
    const limitMb = Math.round(MAX_VOLUME_BYTES / (1024 * 1024));
    loadFullDetailBtn.textContent = `Full detail unavailable (${mb} MB exceeds ${limitMb} MB limit)`;
  } else {
    loadFullDetailBtn.disabled = false;
    loadFullDetailBtn.textContent = "Load full detail (level 0)";
  }
}

function setMode(next: ViewMode): void {
  mode = next;
  modeSlicesBtn.classList.toggle("active", mode === "slices");
  modeVolumeBtn.classList.toggle("active", mode === "volume");
  canvas.style.display = mode === "slices" ? "block" : "none";
  volumeCanvas.style.display = mode === "volume" ? "block" : "none";
  document.querySelector<HTMLDivElement>("#overlay")!.style.display =
    mode === "slices" ? "flex" : "none";
  volumeOverlay.style.display = mode === "volume" ? "flex" : "none";
  volumeSection.style.display = mode === "volume" ? "block" : "none";
  sliceHint.style.display = mode === "slices" ? "block" : "none";
  volumeHint.style.display = mode === "volume" ? "block" : "none";
  updateWindowPresetsVisibility();

  if (mode === "volume" && currentDetail) {
    void loadVolumeIfNeeded(currentDetail.series_uid, loadedVolume?.level ?? 1).then(() => {
      volumeView.render();
    });
  }
}

async function loadSeries(seriesUid: string): Promise<void> {
  emptyState.style.display = "none";
  const detail = await fetchSeriesDetail(seriesUid);
  currentDetail = detail;
  currentOrdinal = 0;
  sliceCache = new Map();
  inFlight = new Map();
  loadedVolume = null;
  volumeInFlight = null;

  infoPatient.textContent = detail.patient_id ?? "—";
  infoModality.textContent = detail.modality ?? "—";
  infoDims.textContent = `${detail.rows} x ${detail.cols}`;
  infoSlices.textContent = String(detail.slice_count);

  uncalibratedNotice.style.display = detail.hu_calibrated ? "none" : "block";
  updateFullDetailButton();

  currentWindow = { ...PRESETS.softTissue };
  highlightActivePreset(null);
  updateWindowLabel();
  volumeView.setWindow(currentWindow);

  await showSlice(0);

  if (mode === "volume") {
    await loadVolumeIfNeeded(seriesUid, 1);
    volumeView.render();
  }
}

function wireInteractions(): void {
  canvas.addEventListener(
    "wheel",
    (ev) => {
      if (!currentDetail) return;
      ev.preventDefault();
      const dir = ev.deltaY > 0 ? 1 : -1;
      const next = Math.min(currentDetail.slice_count - 1, Math.max(0, currentOrdinal + dir));
      if (next !== currentOrdinal) void showSlice(next);
    },
    { passive: false }
  );

  let dragging = false;
  let lastX = 0;
  let lastY = 0;
  canvas.addEventListener("mousedown", (ev) => {
    if (ev.button !== 0) return;
    dragging = true;
    lastX = ev.clientX;
    lastY = ev.clientY;
  });
  window.addEventListener("mousemove", (ev) => {
    if (!dragging) return;
    const dx = ev.clientX - lastX;
    const dy = ev.clientY - lastY;
    lastX = ev.clientX;
    lastY = ev.clientY;
    // Standard radiology drag: horizontal moves the window centre,
    // vertical moves the window width. Sensitivity is in HU per pixel.
    currentWindow = {
      center: currentWindow.center + dx * 3,
      width: Math.max(1, currentWindow.width + dy * 4),
    };
    highlightActivePreset(null);
    updateWindowLabel();
    view.setWindow(currentWindow);
    view.render();
    volumeView.setWindow(currentWindow);
  });
  window.addEventListener("mouseup", () => {
    dragging = false;
  });

  window.addEventListener("resize", () => {
    view.render();
    if (mode === "volume") volumeView.render();
  });

  // Volume camera: left-drag orbits, wheel zooms. No roll; pitch is clamped
  // inside VolumeView.orbit to avoid gimbal flip at the poles.
  let orbiting = false;
  let orbitLastX = 0;
  let orbitLastY = 0;
  volumeCanvas.addEventListener("mousedown", (ev) => {
    if (ev.button !== 0) return;
    orbiting = true;
    orbitLastX = ev.clientX;
    orbitLastY = ev.clientY;
  });
  window.addEventListener("mousemove", (ev) => {
    if (!orbiting) return;
    const dx = ev.clientX - orbitLastX;
    const dy = ev.clientY - orbitLastY;
    orbitLastX = ev.clientX;
    orbitLastY = ev.clientY;
    volumeView.orbit(-dx * 0.008, dy * 0.008);
    volumeView.render();
  });
  window.addEventListener("mouseup", () => {
    orbiting = false;
  });
  volumeCanvas.addEventListener(
    "wheel",
    (ev) => {
      ev.preventDefault();
      const factor = ev.deltaY > 0 ? 1.1 : 1 / 1.1;
      volumeView.zoom(factor);
      volumeView.render();
    },
    { passive: false }
  );

  modeSlicesBtn.addEventListener("click", () => setMode("slices"));
  modeVolumeBtn.addEventListener("click", () => setMode("volume"));

  qualitySlider.addEventListener("input", () => {
    const steps = Number(qualitySlider.value);
    volumeView.setSteps(steps);
    qualityLabel.textContent = String(steps);
    if (mode === "volume") volumeView.render();
  });

  opacitySlider.addEventListener("input", () => {
    const scale = Number(opacitySlider.value) / 100;
    volumeView.setOpacityScale(scale);
    opacityLabel.textContent = scale.toFixed(2);
    if (mode === "volume") volumeView.render();
  });

  thresholdSlider.addEventListener("input", () => {
    const hu = Number(thresholdSlider.value);
    volumeView.setThresholdHU(hu);
    thresholdLabel.textContent = `${hu} HU`;
    if (mode === "volume") volumeView.render();
  });

  mipToggle.addEventListener("change", () => {
    volumeView.setMipMode(mipToggle.checked);
    updateWindowPresetsVisibility();
    if (mode === "volume") volumeView.render();
  });

  loadFullDetailBtn.addEventListener("click", () => {
    if (!currentDetail) return;
    void loadVolumeIfNeeded(currentDetail.series_uid, 0).then(() => volumeView.render());
  });
}

function initVolumeControls(): void {
  buildTfPresetButtons();
  highlightActiveTfPreset(currentTfPreset);
  applyTransferFunctionPreset();
  volumeView.setSteps(Number(qualitySlider.value));
  qualityLabel.textContent = qualitySlider.value;
  volumeView.setOpacityScale(Number(opacitySlider.value) / 100);
  opacityLabel.textContent = (Number(opacitySlider.value) / 100).toFixed(2);
  volumeView.setThresholdHU(Number(thresholdSlider.value));
  thresholdLabel.textContent = `${thresholdSlider.value} HU`;
}

async function init(): Promise<void> {
  buildPresetButtons();
  initVolumeControls();
  wireInteractions();
  setMode("slices");

  let list: SeriesSummary[] = [];
  try {
    list = await fetchSeriesList();
  } catch (err) {
    seriesSelect.innerHTML = "";
    const opt = document.createElement("option");
    opt.textContent = "Failed to load series list";
    seriesSelect.appendChild(opt);
    console.error(err);
    return;
  }

  seriesSelect.innerHTML = "";
  if (list.length === 0) {
    const opt = document.createElement("option");
    opt.textContent = "No series available";
    seriesSelect.appendChild(opt);
    return;
  }

  for (const s of list) {
    const opt = document.createElement("option");
    opt.value = s.series_uid;
    opt.textContent = `${s.patient_id} — ${s.modality} (${s.slice_count} slices)`;
    seriesSelect.appendChild(opt);
  }

  seriesSelect.addEventListener("change", () => {
    if (seriesSelect.value) void loadSeries(seriesSelect.value);
  });

  await loadSeries(list[0].series_uid);
}

void init();
