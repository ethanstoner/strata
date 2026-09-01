import { PRESETS, type Window } from "./windowing";
import { fetchSeriesList, fetchSeriesDetail, fetchSlice } from "./api";
import type { SeriesSummary, SeriesDetail, SliceData } from "./api";
import { SliceView } from "./sliceview";

const canvas = document.querySelector<HTMLCanvasElement>("#gl-canvas")!;
const view = new SliceView(canvas);

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

let currentDetail: SeriesDetail | null = null;
let currentOrdinal = 0;
let currentWindow: Window = { ...PRESETS.softTissue };
let sliceCache = new Map<number, SliceData>();
let inFlight = new Map<number, Promise<SliceData>>();
const PREFETCH_RADIUS = 3;

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
    });
    presetsContainer.appendChild(btn);
  }
}

async function loadSeries(seriesUid: string): Promise<void> {
  emptyState.style.display = "none";
  const detail = await fetchSeriesDetail(seriesUid);
  currentDetail = detail;
  currentOrdinal = 0;
  sliceCache = new Map();
  inFlight = new Map();

  infoPatient.textContent = detail.patient_id ?? "—";
  infoModality.textContent = detail.modality ?? "—";
  infoDims.textContent = `${detail.rows} x ${detail.cols}`;
  infoSlices.textContent = String(detail.slice_count);

  uncalibratedNotice.style.display = detail.hu_calibrated ? "none" : "block";

  currentWindow = { ...PRESETS.softTissue };
  highlightActivePreset(null);
  updateWindowLabel();

  await showSlice(0);
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
  });
  window.addEventListener("mouseup", () => {
    dragging = false;
  });

  window.addEventListener("resize", () => view.render());
}

async function init(): Promise<void> {
  buildPresetButtons();
  wireInteractions();

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
