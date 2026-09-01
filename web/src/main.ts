import { PRESETS, type Window } from "./windowing";
import { fetchSeriesList, fetchSeriesDetail, fetchSlice, fetchVolume, ApiError } from "./api";
import type { SeriesSummary, SeriesDetail, SliceData, VolumeData } from "./api";
import { SliceView } from "./sliceview";
import { VolumeView } from "./volumeview";
import { buildTransferFunctionLUT, TRANSFER_PRESETS } from "./transferfunction";
import { formatSeriesOption } from "./seriespicker";
import {
  DEFAULT_HU_RANGE,
  requiredSteps,
  MAX_RAYMARCH_STEPS,
  MAX_VOLUME_BYTES,
  computeLevelOptions,
  chooseDefaultLevel,
  type HuRange,
} from "./volumemath";

// Extra samples per voxel along the worst-case ray, beyond the bare Nyquist
// floor (oversample=1.0). 1.0 still visibly aliases in practice since
// samples rarely land on voxel centres; 1.5 cleans that up without doubling
// per-frame cost the way 2.0 would.
const STEP_OVERSAMPLE = 1.5;

type ViewMode = "slices" | "volume";

const canvas = document.querySelector<HTMLCanvasElement>("#gl-canvas")!;
const volumeCanvas = document.querySelector<HTMLCanvasElement>("#gl-canvas-volume")!;

const seriesSelect = document.querySelector<HTMLSelectElement>("#series-select")!;
const emptyState = document.querySelector<HTMLDivElement>("#empty-state")!;
const errorState = document.querySelector<HTMLDivElement>("#error-state")!;
const errorMessageEl = document.querySelector<HTMLParagraphElement>("#error-message")!;
const errorRetryBtn = document.querySelector<HTMLButtonElement>("#error-retry")!;
const seriesErrorEl = document.querySelector<HTMLDivElement>("#series-error")!;
const uncalibratedNotice = document.querySelector<HTMLDivElement>("#uncalibrated-notice")!;
const sliceLabel = document.querySelector<HTMLSpanElement>("#slice-label")!;
const windowLabel = document.querySelector<HTMLSpanElement>("#window-label")!;
const infoPatient = document.querySelector<HTMLElement>("#info-patient")!;
const infoStudy = document.querySelector<HTMLElement>("#info-study")!;
const infoModality = document.querySelector<HTMLElement>("#info-modality")!;
const infoDims = document.querySelector<HTMLElement>("#info-dims")!;
const infoSlices = document.querySelector<HTMLElement>("#info-slices")!;
const presetsContainer = document.querySelector<HTMLDivElement>("#presets")!;
const warningsSection = document.querySelector<HTMLDivElement>("#warnings-section")!;
const warningsToggle = document.querySelector<HTMLButtonElement>("#warnings-toggle")!;
const warningsList = document.querySelector<HTMLUListElement>("#warnings-list")!;

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
const volumeErrorEl = document.querySelector<HTMLDivElement>("#volume-error")!;
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
const levelOptionsContainer = document.querySelector<HTMLDivElement>("#level-options")!;

// WebGL2 is a hard requirement for both views (integer textures for slices,
// sampler3D + raymarching for volume). Detect and explain before ever
// touching the canvases, rather than letting SliceView/VolumeView throw
// mid-construction and leave a blank/black page with only a console error.
let view: SliceView;
let volumeView: VolumeView;
try {
  view = new SliceView(canvas);
  volumeView = new VolumeView(volumeCanvas);
} catch (err) {
  console.error(err);
  showFatalError(
    "This browser or GPU does not support WebGL2, which Strata requires to render medical images. Try a recent version of Chrome, Firefox, or Edge with hardware acceleration enabled.",
    false
  );
  throw err;
}

let currentDetail: SeriesDetail | null = null;
let currentOrdinal = 0;
let currentWindow: Window = { ...PRESETS.softTissue };
let sliceCache = new Map<number, SliceData>();
let inFlight = new Map<number, Promise<SliceData>>();
const PREFETCH_RADIUS = 3;

let mode: ViewMode = "slices";
let loadedVolume: { seriesUid: string; level: number } | null = null;
// The volume request currently in flight, if any. Carries which series/level
// it is for, not just the bare promise: a response may only be applied to the
// view if it is still the request the UI is waiting on. Without that identity
// check, a slow volume fetch for a previously-selected series can resolve
// after a newer series' volume has already loaded and overwrite it — leaving
// one patient's anatomy rendered under another patient's name.
let volumeRequest: { seriesUid: string; level: number; promise: Promise<VolumeData> } | null =
  null;
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

async function loadVolumeIfNeeded(seriesUid: string, level: number): Promise<void> {
  if (loadedVolume && loadedVolume.seriesUid === seriesUid && loadedVolume.level === level) {
    return;
  }
  // The same series/level is already being fetched: wait for that request
  // rather than duplicating it. Its own continuation applies the result.
  if (volumeRequest && volumeRequest.seriesUid === seriesUid && volumeRequest.level === level) {
    await volumeRequest.promise.catch(() => {
      /* the owning continuation reported the failure */
    });
    return;
  }

  volumeLoading.style.display = "block";
  volumeErrorEl.style.display = "none";
  const request = { seriesUid, level, promise: fetchVolume(seriesUid, level) };
  volumeRequest = request;
  try {
    const volume = await request.promise;
    // Superseded while in flight (the user switched series, or a newer
    // request replaced this one): discard the response instead of uploading
    // a volume the UI no longer describes.
    if (volumeRequest !== request) return;

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
  } catch (err) {
    // A superseded request's failure is not the current view's failure;
    // the newer request reports its own outcome.
    if (volumeRequest !== request) return;
    console.error(err);
    volumeErrorEl.textContent =
      err instanceof ApiError ? err.message : "Failed to load this volume level.";
    volumeErrorEl.style.display = "block";
  } finally {
    if (volumeRequest === request) {
      volumeRequest = null;
    }
    // Only the request that still owns the loading UI may tear it down —
    // a stale continuation must not hide the spinner for a newer fetch
    // that is still in flight.
    if (volumeRequest === null) {
      volumeLoading.style.display = "none";
      renderLevelOptions();
    }
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

function formatMb(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  return mb < 10 ? `${mb.toFixed(1)} MB` : `${Math.round(mb)} MB`;
}

/** Rebuilds the pyramid level selector for the current series, showing real dims/size per level and disabling any level the server would 400 on. */
function renderLevelOptions(): void {
  levelOptionsContainer.innerHTML = "";
  if (!currentDetail) return;
  const detail = currentDetail;
  const limitMb = Math.round(MAX_VOLUME_BYTES / (1024 * 1024));
  const options = computeLevelOptions(detail.cols, detail.rows, detail.slice_count);

  for (const opt of options) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "level-option";
    btn.disabled = !opt.available;
    const isActive =
      !!loadedVolume && loadedVolume.seriesUid === detail.series_uid && loadedVolume.level === opt.level;
    btn.classList.toggle("active", isActive);

    const row = document.createElement("div");
    row.className = "level-row";
    const main = document.createElement("span");
    main.textContent = `Level ${opt.level} — ${opt.dims.x}x${opt.dims.y}x${opt.dims.z}`;
    const size = document.createElement("span");
    size.textContent = formatMb(opt.bytes);
    row.append(main, size);
    btn.appendChild(row);

    if (!opt.available) {
      const reason = document.createElement("span");
      reason.className = "level-reason";
      reason.textContent = `unavailable — exceeds ${limitMb} MB limit`;
      btn.appendChild(reason);
    }

    btn.addEventListener("click", () => {
      if (volumeRequest) return; // one volume fetch at a time
      void loadVolumeIfNeeded(detail.series_uid, opt.level).then(() => {
        if (mode === "volume") volumeView.render();
      });
    });
    levelOptionsContainer.appendChild(btn);
  }
}

/** Shows/hides the non-alarming scan-warnings disclosure; renders nothing when there are none. */
function renderWarnings(warnings: string[]): void {
  warningsList.innerHTML = "";
  if (warnings.length === 0) {
    warningsSection.style.display = "none";
    return;
  }
  warningsSection.style.display = "block";
  warningsList.style.display = "none";
  warningsToggle.textContent = `${warnings.length} scan warning${warnings.length === 1 ? "" : "s"}`;
  for (const w of warnings) {
    const li = document.createElement("li");
    li.textContent = w;
    warningsList.appendChild(li);
  }
}

/** Shows whichever canvas matches `mode`, hides the other. Called after anything (an empty/error state) may have hidden both, so real content coming back reveals the right one. */
function applyCanvasVisibility(): void {
  canvas.style.display = mode === "slices" ? "block" : "none";
  volumeCanvas.style.display = mode === "volume" ? "block" : "none";
}

/**
 * Full-viewport fatal error (unreachable server, missing WebGL2): overlays
 * #error-state absolutely over the viewport and hides both canvases — there
 * is nothing to render, and leaving a canvas in normal flow after this
 * element would push it below the fold (it's a static-flow sibling of a
 * viewport-filling canvas otherwise; see the CSS comment on #empty-state).
 */
function showFatalError(message: string, retryable: boolean, onRetry?: () => void): void {
  emptyState.style.display = "none";
  canvas.style.display = "none";
  volumeCanvas.style.display = "none";
  errorMessageEl.textContent = message;
  if (retryable && onRetry) {
    errorRetryBtn.style.display = "inline-block";
    errorRetryBtn.onclick = () => {
      hideFatalError();
      onRetry();
    };
  } else {
    errorRetryBtn.style.display = "none";
    errorRetryBtn.onclick = null;
  }
  errorState.style.display = "flex";
  seriesSelect.style.display = "none";
}

function hideFatalError(): void {
  errorState.style.display = "none";
  seriesSelect.style.display = "";
  // Canvases stay hidden until a series actually loads (loadSeries calls
  // applyCanvasVisibility on success) — nothing to render yet, and the
  // black #viewport background looks identical either way in the gap.
}

function setMode(next: ViewMode): void {
  mode = next;
  modeSlicesBtn.classList.toggle("active", mode === "slices");
  modeVolumeBtn.classList.toggle("active", mode === "volume");
  // Only actually shows a canvas if a series is loaded; loadSeries hid both
  // while empty/erroring and is the one that reveals them again.
  if (currentDetail) applyCanvasVisibility();
  document.querySelector<HTMLDivElement>("#overlay")!.style.display =
    mode === "slices" ? "flex" : "none";
  volumeOverlay.style.display = mode === "volume" ? "flex" : "none";
  volumeSection.style.display = mode === "volume" ? "block" : "none";
  sliceHint.style.display = mode === "slices" ? "block" : "none";
  volumeHint.style.display = mode === "volume" ? "block" : "none";
  updateWindowPresetsVisibility();

  if (mode === "volume" && currentDetail) {
    const detail = currentDetail;
    const level =
      loadedVolume?.level ?? chooseDefaultLevel(detail.cols, detail.rows, detail.slice_count);
    void loadVolumeIfNeeded(detail.series_uid, level).then(() => {
      volumeView.render();
    });
  }
}

async function loadSeries(seriesUid: string): Promise<void> {
  seriesErrorEl.style.display = "none";
  try {
    const detail = await fetchSeriesDetail(seriesUid);
    currentDetail = detail;
    currentOrdinal = 0;
    sliceCache = new Map();
    inFlight = new Map();
    loadedVolume = null;
    // Orphan any volume fetch still in flight for the previous series: its
    // continuation sees it is no longer the current request and discards
    // the response rather than rendering it under this series' labels.
    volumeRequest = null;

    emptyState.style.display = "none";
    applyCanvasVisibility();
    infoPatient.textContent = detail.patient_id ?? "—";
    infoStudy.textContent = detail.study_description ?? "—";
    infoModality.textContent = detail.modality ?? "—";
    infoDims.textContent = `${detail.rows} x ${detail.cols}`;
    infoSlices.textContent = String(detail.slice_count);

    uncalibratedNotice.style.display = detail.hu_calibrated ? "none" : "block";
    renderWarnings(detail.warnings);
    renderLevelOptions();

    currentWindow = { ...PRESETS.softTissue };
    highlightActivePreset(null);
    updateWindowLabel();
    volumeView.setWindow(currentWindow);

    await showSlice(0);

    if (mode === "volume") {
      const level = chooseDefaultLevel(detail.cols, detail.rows, detail.slice_count);
      await loadVolumeIfNeeded(seriesUid, level);
      volumeView.render();
    }
  } catch (err) {
    // Deliberately doesn't touch currentDetail/loadedVolume on failure: if a
    // series was already loaded, it stays on screen and usable, with only
    // this banner reporting the new selection's failure.
    console.error(err);
    const reason = err instanceof ApiError ? err.message : "Failed to load this series.";
    seriesErrorEl.textContent = `Could not load series: ${reason}`;
    seriesErrorEl.style.display = "block";
    if (!currentDetail) {
      // Nothing has ever loaded: replace the (otherwise blank, unrendered)
      // canvas with the empty state rather than leaving it in the flow.
      canvas.style.display = "none";
      volumeCanvas.style.display = "none";
      emptyState.textContent = "Select a series to begin";
      emptyState.style.display = "flex";
    }
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

  warningsToggle.addEventListener("click", () => {
    warningsList.style.display = warningsList.style.display === "none" ? "block" : "none";
  });

  seriesSelect.addEventListener("change", () => {
    if (seriesSelect.value) void loadSeries(seriesSelect.value);
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

async function loadSeriesListAndFirst(): Promise<void> {
  hideFatalError();
  seriesSelect.disabled = false;
  seriesSelect.innerHTML = "";
  const loadingOpt = document.createElement("option");
  loadingOpt.textContent = "Loading series…";
  seriesSelect.appendChild(loadingOpt);

  let list: SeriesSummary[] = [];
  try {
    list = await fetchSeriesList();
  } catch (err) {
    console.error(err);
    const message =
      err instanceof ApiError
        ? `The server responded with an error: ${err.message}`
        : "Could not reach the strata server. Make sure it is running and reachable, then retry.";
    showFatalError(message, true, () => void loadSeriesListAndFirst());
    return;
  }

  seriesSelect.innerHTML = "";
  if (list.length === 0) {
    const opt = document.createElement("option");
    opt.textContent = "No series available";
    seriesSelect.appendChild(opt);
    seriesSelect.disabled = true;
    canvas.style.display = "none";
    volumeCanvas.style.display = "none";
    emptyState.textContent =
      "No DICOM series found. Point strata at a folder containing DICOM files, or run scripts/fetch-sample.ps1 to download a sample study.";
    emptyState.style.display = "flex";
    return;
  }

  for (const s of list) {
    const opt = document.createElement("option");
    opt.value = s.series_uid;
    opt.textContent = formatSeriesOption(s);
    seriesSelect.appendChild(opt);
  }

  await loadSeries(list[0].series_uid);
}

async function init(): Promise<void> {
  buildPresetButtons();
  initVolumeControls();
  wireInteractions();
  setMode("slices");
  await loadSeriesListAndFirst();
}

void init();
