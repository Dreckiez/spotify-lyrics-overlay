import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  availableMonitors,
  currentMonitor,
  getCurrentWindow,
  LogicalSize,
  PhysicalPosition,
} from "@tauri-apps/api/window";

const DEFAULT_CONFIG = {
  text_color: "#e9edfa",
  active_color: "#68f1ce",
  background_opacity: 24,
  font_family: "Segoe UI",
  font_size: 22,
  alignment: "left",
  offset_ms: 0,
  app_scale: 100,
};
const BASE_WINDOW_WIDTH = 470;
const BASE_WINDOW_HEIGHT = 500;
const FREE_POSITION_KEY = "spotify-lyrics-overlay-free-position";

const elements = {
  root: document.documentElement,
  overlay: document.querySelector("#overlay"),
  trackTitle: document.querySelector("#track-title"),
  trackArtist: document.querySelector("#track-artist"),
  trackSeparator: document.querySelector("#track-separator"),
  playbackState: document.querySelector("#playback-state"),
  lyricsList: document.querySelector("#lyrics-list"),
  settingsStatus: document.querySelector("#settings-status"),
  settingsPanel: document.querySelector("#settings-panel"),
  settingsForm: document.querySelector("#settings-form"),
  clickThroughButton: document.querySelector("#click-through-button"),
  clickThroughIcon: document.querySelector("#click-through-icon"),
};

const IDLE_TIMEOUT_MS = 3000;
const LEAVE_TIMEOUT_MS = 1500;
let idleTimer = null;

function resetIdleTimer(timeout = IDLE_TIMEOUT_MS) {
  if (idleTimer) {
    clearTimeout(idleTimer);
    idleTimer = null;
  }
  elements.overlay.classList.remove("idle");

  if (elements.settingsPanel.hidden && !clickThrough) {
    idleTimer = setTimeout(() => {
      elements.overlay.classList.add("idle");
    }, timeout);
  }
}

let currentLines = [];
let currentIndex = null;
let currentLineSignature = "";
let currentTrackKey = "";
let pendingTrackTopReset = false;
let currentConfig = { ...DEFAULT_CONFIG };
let clickThrough = false;

function scrollToTop(smooth = false) {
  elements.lyricsList.scrollTo({
    top: 0,
    behavior: smooth ? "smooth" : "instant",
  });
}

function renderLines(lines, activeIndex, lyricsState, startAtTop = false) {
  elements.lyricsList.replaceChildren();

  if (!lines?.length) {
    const empty = document.createElement("p");
    empty.className = "empty-state";
    empty.textContent = lyricsState === "loading"
      ? "Đang tìm lời bài hát…"
      : lyricsState === "error"
        ? "Không kết nối được LRCLIB. Thử lại khi phát bài tiếp theo."
        : lyricsState === "not-found"
          ? "♪ Chưa tìm thấy lời đồng bộ cho bài hát này."
          : "Mở Spotify và phát một bài hát để bắt đầu.";
    elements.lyricsList.append(empty);
    scrollToTop(false);
    return;
  }

  const fragment = document.createDocumentFragment();
  for (let index = 0; index < lines.length; index += 1) {
    const paragraph = document.createElement("p");
    paragraph.className = "lyric-line";
    if (index === activeIndex) paragraph.classList.add("active");
    else if (activeIndex != null && Math.abs(index - activeIndex) === 1) paragraph.classList.add("near");
    paragraph.dataset.index = String(index);
    paragraph.textContent = lines[index].text;
    fragment.append(paragraph);
  }
  elements.lyricsList.append(fragment);

  if (startAtTop || activeIndex == null || activeIndex <= 0) {
    scrollToTop(false);
    requestAnimationFrame(() => scrollToTop(false));
  } else {
    requestAnimationFrame(() => {
      elements.lyricsList.querySelector(".active")?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
  }
}

function updateActiveLine(activeIndex) {
  const children = elements.lyricsList.children;
  for (let i = 0; i < children.length; i += 1) {
    const el = children[i];
    if (!el.classList.contains("lyric-line")) continue;
    const idx = Number(el.dataset.index);
    if (idx === activeIndex) {
      el.classList.add("active");
      el.classList.remove("near");
    } else if (activeIndex != null && Math.abs(idx - activeIndex) === 1) {
      el.classList.add("near");
      el.classList.remove("active");
    } else {
      el.classList.remove("active", "near");
    }
  }

  if (activeIndex == null || activeIndex <= 0) {
    scrollToTop(true);
  } else {
    const activeEl = elements.lyricsList.querySelector(`.lyric-line[data-index="${activeIndex}"]`);
    activeEl?.scrollIntoView({ behavior: "smooth", block: "center" });
  }
}

async function applyWindowScale(scalePercent) {
  const scale = Number(scalePercent) || 100;
  const factor = scale / 100;
  document.documentElement.style.zoom = String(factor);

  try {
    const window = getCurrentWindow();
    const targetWidth = Math.round(BASE_WINDOW_WIDTH * factor);
    const targetHeight = Math.round(BASE_WINDOW_HEIGHT * factor);
    await window.setSize(new LogicalSize(targetWidth, targetHeight));

    const monitor = await currentMonitor();
    if (monitor) {
      const windowPos = await window.outerPosition();
      const windowSize = await window.outerSize();
      const maxX = monitor.position.x + Math.max(0, monitor.size.width - windowSize.width);
      const maxY = monitor.position.y + Math.max(0, monitor.size.height - windowSize.height);
      const clampedX = Math.max(monitor.position.x, Math.min(windowPos.x, maxX));
      const clampedY = Math.max(monitor.position.y, Math.min(windowPos.y, maxY));
      if (clampedX !== windowPos.x || clampedY !== windowPos.y) {
        await window.setPosition(new PhysicalPosition(clampedX, clampedY));
      }
    }
  } catch (error) {
    console.error("Could not resize window for scale:", error);
  }
}

function applyConfig(config) {
  currentConfig = { ...DEFAULT_CONFIG, ...config };
  elements.root.style.setProperty("--text-color", currentConfig.text_color);
  elements.root.style.setProperty("--active-color", currentConfig.active_color);
  elements.root.style.setProperty("--font-size", `${currentConfig.font_size}px`);
  elements.root.style.setProperty("--font-family", currentConfig.font_family);
  elements.root.style.setProperty("--alignment", currentConfig.alignment);
  elements.root.style.setProperty(
    "--panel-background",
    `rgba(13, 18, 31, ${Number(currentConfig.background_opacity) / 100})`,
  );

  void applyWindowScale(currentConfig.app_scale);

  for (const [name, value] of Object.entries(currentConfig)) {
    const input = elements.settingsForm.elements.namedItem(name);
    if (input) input.value = String(value);
  }
  updateOutputs();
}

function updateOutputs() {
  const form = elements.settingsForm.elements;
  document.querySelector("#opacity-output").value = `${form.background_opacity.value}%`;
  document.querySelector("#font-size-output").value = `${form.font_size.value} px`;
  document.querySelector("#scale-output").value = `${form.app_scale.value}%`;
  document.querySelector("#offset-output").value = `${form.offset_ms.value} ms`;
}

async function snapWindowToPosition(position) {
  const window = getCurrentWindow();
  const monitor = await currentMonitor();
  if (!monitor) return;

  const windowSize = await window.outerSize();
  const screen = { position: monitor.position, size: monitor.size };
  const margin = 0;
  const left = Math.min(
    screen.position.x + margin,
    screen.position.x + Math.max(0, screen.size.width - windowSize.width),
  );
  const right = Math.max(
    screen.position.x,
    screen.position.x + screen.size.width - windowSize.width - margin,
  );
  const top = Math.min(
    screen.position.y + margin,
    screen.position.y + Math.max(0, screen.size.height - windowSize.height),
  );
  const bottom = Math.max(
    screen.position.y,
    screen.position.y + screen.size.height - windowSize.height - margin,
  );

  const x = position.endsWith("right") ? right : left;
  const y = position.startsWith("bottom") ? bottom : top;
  await window.setPosition(new PhysicalPosition(x, y));
}

function storeFreePosition(position) {
  try {
    localStorage.setItem(FREE_POSITION_KEY, JSON.stringify({ x: position.x, y: position.y }));
  } catch (error) {
    console.error("Could not remember the overlay position:", error);
  }
}

async function restoreFreePosition() {
  let savedPosition;
  try {
    savedPosition = JSON.parse(localStorage.getItem(FREE_POSITION_KEY) || "null");
  } catch {
    return;
  }
  if (!Number.isFinite(savedPosition?.x) || !Number.isFinite(savedPosition?.y)) return;

  const window = getCurrentWindow();
  const monitors = await availableMonitors();
  const monitor = monitors.find((candidate) => {
    const { x, y } = candidate.position;
    return savedPosition.x >= x
      && savedPosition.x < x + candidate.size.width
      && savedPosition.y >= y
      && savedPosition.y < y + candidate.size.height;
  }) || await currentMonitor();
  if (!monitor) return;

  const windowSize = await window.outerSize();
  const maxX = monitor.position.x + Math.max(0, monitor.size.width - windowSize.width);
  const maxY = monitor.position.y + Math.max(0, monitor.size.height - windowSize.height);
  const x = Math.max(monitor.position.x, Math.min(savedPosition.x, maxX));
  const y = Math.max(monitor.position.y, Math.min(savedPosition.y, maxY));
  await window.setPosition(new PhysicalPosition(x, y));
}

async function trackFreePosition() {
  await getCurrentWindow().onMoved(({ payload }) => {
    storeFreePosition(payload);
  });
}

function showSettings(show = true) {
  elements.settingsPanel.hidden = !show;
  elements.overlay.classList.toggle("settings-open", show);
  if (show) {
    elements.overlay.classList.remove("idle");
    if (idleTimer) {
      clearTimeout(idleTimer);
      idleTimer = null;
    }
  } else {
    resetIdleTimer();
  }
  if (show && clickThrough) {
    void setClickThrough(false);
  }
}

function setClickThroughIndicator(enabled) {
  clickThrough = enabled;
  elements.clickThroughButton.setAttribute("aria-pressed", String(enabled));
  elements.clickThroughIcon.textContent = enabled ? "⊘" : "◉";
  elements.clickThroughButton.title = enabled ? "Xuyên chuột đang bật (Ctrl + Shift + L để mở lại)" : "Bật xuyên chuột";
  elements.clickThroughButton.setAttribute("aria-label", elements.clickThroughButton.title);

  if (enabled) {
    elements.overlay.classList.add("idle");
    if (idleTimer) {
      clearTimeout(idleTimer);
      idleTimer = null;
    }
  } else {
    elements.overlay.classList.remove("idle");
    resetIdleTimer();
  }
}

async function setClickThrough(enabled) {
  try {
    const actual = await invoke("set_click_through", { enabled });
    setClickThroughIndicator(actual);
  } catch (error) {
    elements.settingsStatus.textContent = `Không đổi được chế độ chuột: ${error}`;
  }
}

function updateTick(tick) {
  const nextTrackKey = tick.title ? `${tick.title}\u0000${tick.artist || ""}` : "";
  if (nextTrackKey !== currentTrackKey) {
    currentTrackKey = nextTrackKey;
    pendingTrackTopReset = true;
    currentLineSignature = "";
    currentLines = [];
    currentIndex = null;
    scrollToTop(false);
  }

  if (!tick.title) {
    elements.trackTitle.textContent = "Đang chờ Spotify";
    elements.trackArtist.textContent = "";
    elements.trackSeparator.hidden = true;
    elements.playbackState.textContent = "● Chờ phát";
    elements.playbackState.classList.remove("playing");
    currentLines = [];
    currentIndex = null;
    currentLineSignature = "";
    renderLines([], null, "idle", true);
    return;
  }

  elements.trackTitle.textContent = tick.title;
  elements.trackArtist.textContent = tick.artist || "";
  elements.trackSeparator.hidden = !tick.artist;
  elements.playbackState.textContent = tick.is_playing ? "● Đang phát" : "Ⅱ Đã tạm dừng";
  elements.playbackState.classList.toggle("playing", Boolean(tick.is_playing));

  const nextLines = tick.lines || [];
  const signature = `${tick.lyrics_state}:${nextLines.map((line) => `${line.timestamp_ms}:${line.text}`).join("\n")}`;
  const linesChanged = signature !== currentLineSignature;
  const indexChanged = tick.active_index !== currentIndex;

  if (linesChanged) {
    currentLineSignature = signature;
    currentLines = nextLines;
    currentIndex = tick.active_index;
    renderLines(currentLines, currentIndex, tick.lyrics_state, pendingTrackTopReset);
    if (nextLines.length || ["not-found", "error"].includes(tick.lyrics_state)) {
      pendingTrackTopReset = false;
    }
  } else if (indexChanged) {
    currentIndex = tick.active_index;
    updateActiveLine(currentIndex);
  }
}

elements.clickThroughButton.addEventListener("click", () => setClickThrough(!clickThrough));
document.querySelector("#settings-button").addEventListener("click", () => showSettings(true));
document.querySelector("#close-settings-button").addEventListener("click", () => showSettings(false));
document.querySelector("#reset-button").addEventListener("click", () => applyConfig(DEFAULT_CONFIG));
for (const button of document.querySelectorAll("[data-snap-position]")) {
  button.addEventListener("click", async () => {
    try {
      await snapWindowToPosition(button.dataset.snapPosition);
      elements.settingsStatus.textContent = "Đã chuyển overlay đến góc đã chọn";
    } catch (error) {
      elements.settingsStatus.textContent = `Không chuyển được vị trí: ${error}`;
    }
  });
}
elements.settingsForm.addEventListener("input", () => {
  updateOutputs();
  const scale = Number(elements.settingsForm.elements.app_scale.value) || 100;
  document.documentElement.style.zoom = String(scale / 100);
});
elements.settingsForm.elements.app_scale.addEventListener("change", () => {
  void applyWindowScale(elements.settingsForm.elements.app_scale.value);
});
elements.settingsForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = new FormData(elements.settingsForm);
  const config = {
    text_color: String(form.get("text_color")),
    active_color: String(form.get("active_color")),
    background_opacity: Number(form.get("background_opacity")),
    font_family: String(form.get("font_family")),
    font_size: Number(form.get("font_size")),
    alignment: String(form.get("alignment")),
    offset_ms: Number(form.get("offset_ms")),
    app_scale: Number(form.get("app_scale")),
  };
  try {
    applyConfig(await invoke("save_config", { config }));
    elements.settingsStatus.textContent = "Đã lưu cài đặt";
    showSettings(false);
  } catch (error) {
    elements.settingsStatus.textContent = `Không lưu được cài đặt: ${error}`;
  }
});

window.addEventListener("mousemove", () => resetIdleTimer(IDLE_TIMEOUT_MS));
window.addEventListener("mouseenter", () => resetIdleTimer(IDLE_TIMEOUT_MS));
window.addEventListener("mousedown", () => resetIdleTimer(IDLE_TIMEOUT_MS));
window.addEventListener("mouseleave", () => resetIdleTimer(LEAVE_TIMEOUT_MS));

await listen("lyric-tick", ({ payload }) => updateTick(payload));
await listen("app-config", ({ payload }) => applyConfig(payload));
await listen("click-through-changed", ({ payload }) => setClickThroughIndicator(payload));
await listen("open-settings", () => showSettings(true));

try {
  await getCurrentWindow().setSkipTaskbar(true);
} catch (error) {
  console.error("Could not set skip taskbar:", error);
}

try {
  const config = await invoke("get_config");
  applyConfig(config);
  try {
    await restoreFreePosition();
  } catch (error) {
    console.error("Could not apply the saved overlay position:", error);
  }
  try {
    await trackFreePosition();
  } catch (error) {
    console.error("Could not track overlay movement:", error);
  }
} catch (error) {
  applyConfig(DEFAULT_CONFIG);
  elements.settingsStatus.textContent = `Đang dùng cài đặt mặc định (${error})`;
}

renderLines([], null, "idle");
resetIdleTimer();