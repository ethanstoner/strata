import { PRESETS, type Window } from "./windowing";
import { fetchSeriesList, fetchSeriesDetail, fetchSlice, fetchVolume } from "./api";
import type { SeriesSummary, SeriesDetail, SliceData, VolumeData } from "./api";
import { SliceView } from "./sliceview";
import { VolumeView } from "./volumeview";
import { buildTransferFunctionLUT, TRANSFER_PRESETS } from "./transferfunction";

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
  volumeView.setTransferFunction(buildTransferFunctionLUT(points));
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
    volumeView.uploadVolume(
      volume.voxels,
      volume.dimX,
      volume.dimY,
      volume.dimZ,
      volume.spacingX,
      volume.spacingY,
      volume.spacingZ
    );
    loadedVolume = { seriesUid, level: volume.level };
    volumeUncalibratedNotice.style.display = volume.huCalibrated ? "none" : "block";
    volumeInfoLabel.textContent = `level ${volume.level}  ·  ${volume.dimX}x${volume.dimY}x${volume.dimZ}`;
    volumeView.render();
  } finally {
    volumeLoading.style.display = "none";
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
