// atmosphere · miniplayer.js · Compact always-on-top player that reflows by size: portrait card, wide strip or tiny, with a lyrics or queue panel when there's room
const Mini = {
  on: false,
  restore: null,
  prefs: { cover: true, blur: true, panel: "none", pin: true },
  dragFrom: null,
  layout: "",
  panelShown: false,
  sizeTimer: 0,
  lyricsFor: null,
};

const MINI_MIN = { w: 260, h: 96 };
const MINI_MAX = { w: 900, h: 900 };
const MINI_CORE = 196;
const MINI_STRIP = 112;
const MINI_PANEL = 232;

function miniLoad() {
  try {
    const saved = JSON.parse(localStorage.getItem("atmo.mini") || "{}");
    if (saved.queue && !saved.panel) saved.panel = "queue";
    delete saved.queue;
    Object.assign(Mini.prefs, saved);
  } catch {}
}

function miniSave() {
  try {
    localStorage.setItem("atmo.mini", JSON.stringify(Mini.prefs));
  } catch {}
}

function miniSavedSize() {
  try {
    const s = JSON.parse(localStorage.getItem("atmo.miniSize") || "null");
    if (s && s.w >= MINI_MIN.w && s.h >= MINI_MIN.h) return s;
  } catch {}
  return null;
}

const miniWin = () => window.__TAURI__?.window?.getCurrentWindow?.();

function miniIdealHeight(w, wide) {
  const panel = Mini.prefs.panel !== "none";
  if (wide) return MINI_STRIP + (panel ? MINI_PANEL : 0);
  return MINI_CORE + (Mini.prefs.cover ? Math.min(w - 40, 300) : 0) + (panel ? MINI_PANEL : 0);
}

async function miniSetSize(w, h) {
  const win = miniWin();
  const dpi = window.__TAURI__?.dpi;
  if (!win || !dpi) return;
  const cw = Math.max(MINI_MIN.w, Math.min(MINI_MAX.w, Math.round(w)));
  const ch = Math.max(MINI_MIN.h, Math.min(MINI_MAX.h, Math.round(h)));
  await win.setSize(new dpi.LogicalSize(cw, ch)).catch(() => {});
}

async function miniFit() {
  const w = innerWidth;
  await miniSetSize(w, miniIdealHeight(w, Mini.layout === "wide"));
}

async function openMini() {
  if (Mini.on) return;
  if (typeof closeTv === "function" && Lyr?.tv) await closeTv();
  closeDrawers();
  closeFx();
  Mini.on = true;
  document.body.classList.add("mini-mode");
  const box = $("#mini");
  box.hidden = false;
  const win = miniWin();
  const dpi = window.__TAURI__?.dpi;
  if (win && dpi) {
    Mini.restore = { pos: await win.outerPosition().catch(() => null), size: await win.outerSize().catch(() => null), max: await win.isMaximized().catch(() => false) };
    if (Mini.restore.max) await win.unmaximize().catch(() => {});
    await win.setMaximizable(false).catch(() => {});
    await win.setMinSize(new dpi.LogicalSize(MINI_MIN.w, MINI_MIN.h)).catch(() => {});
    await win.setMaxSize(new dpi.LogicalSize(MINI_MAX.w, MINI_MAX.h)).catch(() => {});
    const saved = miniSavedSize();
    await miniSetSize(saved?.w || 340, saved?.h || miniIdealHeight(340, false));
    await win.setAlwaysOnTop(!!Mini.prefs.pin).catch(() => {});
  }
  buildMini();
}

async function closeMini() {
  if (!Mini.on) return;
  Mini.on = false;
  document.body.classList.remove("mini-mode");
  $("#mini").hidden = true;
  $("#mini").replaceChildren();
  Mini.lyricsFor = null;
  const win = miniWin();
  const dpi = window.__TAURI__?.dpi;
  const r = Mini.restore;
  Mini.restore = null;
  if (!win || !dpi) return;
  await win.setAlwaysOnTop(false).catch(() => {});
  await win.setMaxSize(null).catch(() => {});
  await win.setMaximizable(true).catch(() => {});
  await win.setMinSize(new dpi.LogicalSize(900, 620)).catch(() => {});
  if (r?.size) await win.setSize(new dpi.PhysicalSize(r.size.width, r.size.height)).catch(() => {});
  if (r?.pos) await win.setPosition(new dpi.PhysicalPosition(r.pos.x, r.pos.y)).catch(() => {});
  if (r?.max) await win.maximize().catch(() => {});
}

async function miniToggle(key) {
  if (key === "lyrics" || key === "queue") {
    Mini.prefs.panel = Mini.prefs.panel === key ? "none" : key;
  } else Mini.prefs[key] = !Mini.prefs[key];
  miniSave();
  if (key === "pin") await miniWin()?.setAlwaysOnTop(Mini.prefs.pin).catch(() => {});
  const needsFit = key === "lyrics" || key === "queue" || (key === "cover" && Mini.layout !== "wide");
  buildMini();
  if (needsFit) await miniFit();
}

function miniButton(key, iconName, label, on) {
  return h("button", { class: "icon-btn mini-opt", "aria-pressed": String(on), "aria-label": label, title: label, onclick: () => miniToggle(key) }, icon(iconName));
}

function miniQueueRow(t, i) {
  const row = h("div", { class: "mq-row", draggable: "true", "data-qi": i, ondblclick: () => Player.load(i) },
    h("span", { class: "mq-grip", "aria-hidden": "true" }, icon("grip-vertical")),
    h("div", { class: "lt-main" }, h("span", { class: "lt-title" }, t.title), h("span", { class: "lt-sub" }, t.artist)),
    h("button", { class: "icon-btn", "aria-label": `Remove ${t.title}`, title: "Remove", onclick: (e) => { e.stopPropagation(); Player.removeAt(i); } }, icon("x")));
  row.addEventListener("dragstart", (e) => {
    Mini.dragFrom = i;
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", "");
    row.classList.add("dragging");
  });
  row.addEventListener("dragend", () => row.classList.remove("dragging"));
  row.addEventListener("dragover", (e) => {
    if (Mini.dragFrom == null) return;
    e.preventDefault();
    const r = row.getBoundingClientRect();
    row.classList.toggle("drop-above", e.clientY < r.top + r.height / 2);
    row.classList.toggle("drop-below", e.clientY >= r.top + r.height / 2);
  });
  row.addEventListener("dragleave", () => row.classList.remove("drop-above", "drop-below"));
  row.addEventListener("drop", (e) => {
    e.preventDefault();
    const below = row.classList.contains("drop-below");
    row.classList.remove("drop-above", "drop-below");
    const from = Mini.dragFrom;
    Mini.dragFrom = null;
    if (from == null) return;
    moveInQueue(from, below ? i + 1 : i);
  });
  return row;
}

function moveInQueue(from, to) {
  const q = Player.queue;
  if (from <= Player.index || from >= q.length) return;
  const [t] = q.splice(from, 1);
  const dest = Math.max(Player.index + 1, Math.min(q.length, to > from ? to - 1 : to));
  q.splice(dest, 0, t);
  Player.emit("queue");
  Player.persist();
}

function buildMini() {
  const box = $("#mini");
  if (!box || !Mini.on) return;
  const p = Mini.prefs;
  box.classList.toggle("with-cover", !!p.cover);
  const art = h("div", { class: "mini-art", "data-tauri-drag-region": "" });
  const seek = h("input", { type: "range", class: "range", id: "miniSeek", min: "0", max: "1000", value: "0", "aria-label": "Seek" });
  seek.addEventListener("input", () => {
    rangeFill(seek);
    const d = Player.audio.duration;
    if (isFinite(d)) $("#miniCur").textContent = fmtDur((seek.value / 1000) * d * 1000);
  });
  seek.addEventListener("change", () => Player.seek((seek.value / 1000) * Player.audio.duration));
  box.replaceChildren(
    h("div", { class: "mini-bg", "aria-hidden": "true" }),
    h("div", { class: "mini-top", "data-tauri-drag-region": "" },
      h("span", { class: "mini-brand", "data-tauri-drag-region": "" }, "Atmosphere"),
      h("div", { class: "mini-opts" },
        miniButton("cover", "photo", p.cover ? "Hide cover art" : "Show cover art", p.cover),
        miniButton("blur", "droplet", p.blur ? "Turn off blurred backdrop" : "Blurred cover backdrop", p.blur),
        miniButton("lyrics", "microphone", p.panel === "lyrics" ? "Hide lyrics" : "Show lyrics", p.panel === "lyrics"),
        miniButton("queue", "list", p.panel === "queue" ? "Hide queue" : "Show queue", p.panel === "queue"),
        miniButton("pin", p.pin ? "pinned" : "pin", p.pin ? "Always on top: on" : "Always on top: off", p.pin),
        h("button", { class: "icon-btn", "aria-label": "Back to full window", title: "Back to full window (P)", onclick: closeMini }, icon("arrows-maximize")))),
    h("div", { class: "mini-body", "data-tauri-drag-region": "" },
      art,
      h("div", { class: "mini-info", "data-tauri-drag-region": "" },
        h("div", { class: "mini-meta", "data-tauri-drag-region": "" },
          h("span", { class: "mini-title", id: "miniTitle", "data-tauri-drag-region": "" }),
          h("span", { class: "mini-artist", id: "miniArtist", "data-tauri-drag-region": "" })),
        h("div", { class: "mini-seek" }, h("span", { class: "mini-t", id: "miniCur" }, "0:00"), seek, h("span", { class: "mini-t", id: "miniDur" }, "0:00"))),
      h("div", { class: "mini-ctl" },
        h("button", { class: "icon-btn", "aria-label": "Previous", onclick: () => Player.prev() }, icon("player-skip-back-filled")),
        h("button", { class: "mini-play", id: "miniPlay", onclick: () => Player.toggle() }),
        h("button", { class: "icon-btn", "aria-label": "Next", onclick: () => Player.next() }, icon("player-skip-forward-filled")))),
    h("div", { class: "mini-panel", id: "miniPanel" })
  );
  Mini.lyricsFor = null;
  Mini.layout = "";
  miniLayout();
  updateMini();
  renderMiniPanel();
}

function updateMini() {
  const box = $("#mini");
  if (!box || !Mini.on || !$("#miniTitle")) return;
  const t = Player.current;
  const art = t?.art ? artUrl(t.art) : null;
  box.classList.toggle("with-blur", !!Mini.prefs.blur && !!art);
  box.style.setProperty("--mini-art", art ? `url("${art}")` : "none");
  $("#miniTitle").textContent = t?.title || "Nothing playing";
  $("#miniTitle").title = t?.title || "";
  $("#miniArtist").textContent = t?.artist || "Pick something from your library";
  const artBox = box.querySelector(".mini-art");
  const img = artBox.querySelector("img");
  if (art && img?.getAttribute("src") !== art) artBox.replaceChildren(h("img", { src: art, alt: "", draggable: "false", "data-tauri-drag-region": "" }));
  else if (!art && !artBox.querySelector(".mini-art-empty")) artBox.replaceChildren(h("div", { class: "mini-art-empty", "data-tauri-drag-region": "" }, icon("music")));
  const playing = !Player.audio.paused;
  const pb = $("#miniPlay");
  pb.setAttribute("aria-label", playing ? "Pause" : "Play");
  pb.replaceChildren(icon(playing ? "player-pause-filled" : "player-play-filled"));
  miniTime();
}

function renderMiniPanel() {
  const panel = $("#miniPanel");
  if (!panel || !Mini.on) return;
  const kind = Mini.panelShown ? Mini.prefs.panel : "none";
  panel.dataset.kind = kind;
  if (kind === "queue") {
    Mini.lyricsFor = null;
    const next = Player.queue.slice(Player.index + 1);
    panel.replaceChildren(
      h("div", { class: "mq-head" }, h("span", {}, next.length ? `Up next · ${next.length}` : "Up next"), next.length ? h("button", { class: "btn small ghost", onclick: () => Player.clearUpNext() }, "Clear") : null),
      next.length ? h("div", { class: "mq-list" }, ...next.slice(0, 200).map((q, k) => miniQueueRow(q, Player.index + 1 + k))) : h("p", { class: "mq-empty" }, "Nothing queued. Add songs from your library."));
    return;
  }
  if (kind !== "lyrics") {
    Mini.lyricsFor = null;
    panel.replaceChildren();
    return;
  }
  const t = Player.current;
  if (t && Lyr.trackId !== t.id) loadLyrics();
  const key = `${t?.id || ""}|${Lyr.loading}|${!!Lyr.data}|${Lyr.data?.synced}`;
  if (Mini.lyricsFor === key) return;
  Mini.lyricsFor = key;
  let content;
  if (!t) content = h("p", { class: "mini-lyr-note" }, "Play something to see its lyrics.");
  else if (Lyr.loading) content = h("p", { class: "mini-lyr-note" }, "Finding lyrics…");
  else if (!Lyr.data) content = h("p", { class: "mini-lyr-note" }, "No lyrics found for this song.");
  else {
    content = h("div", { class: "mini-lyr-scroll" }, lyricLines(Lyr.data, false));
    Lyr.active = -1;
    startTick();
  }
  panel.replaceChildren(content);
}

function miniLayout() {
  const box = $("#mini");
  if (!box || !Mini.on) return;
  const w = innerWidth;
  const ht = innerHeight;
  const tiny = w < 290 || ht < 132;
  const wide = !tiny && w >= 400 && w >= ht * 1.3;
  const layout = tiny ? "tiny" : wide ? "wide" : "portrait";
  const panelWant = Mini.prefs.panel !== "none";
  let panelShown = false;
  let artSize = 0;
  if (layout === "wide") {
    panelShown = panelWant && ht >= MINI_STRIP + 120;
    const strip = panelShown ? MINI_STRIP : ht;
    artSize = Math.max(56, Math.min(strip - 24, Math.round(w * 0.32), 300));
  } else if (layout === "portrait") {
    const room = ht - MINI_CORE;
    panelShown = panelWant && room - (Mini.prefs.cover ? 120 : 0) >= 120;
    const forArt = room - (panelShown ? Math.max(MINI_PANEL, room * 0.45) : 0);
    artSize = Mini.prefs.cover ? Math.max(0, Math.min(w - 40, forArt - 14, 420)) : 0;
    if (artSize < 96) artSize = 0;
  }
  box.classList.toggle("wide", layout === "wide");
  box.classList.toggle("tiny", layout === "tiny");
  box.classList.toggle("portrait", layout === "portrait");
  box.classList.toggle("has-panel", panelShown);
  box.classList.toggle("no-art", !Mini.prefs.cover || (layout === "portrait" && !artSize) || layout === "tiny");
  box.style.setProperty("--art", `${artSize}px`);
  const changed = layout !== Mini.layout || panelShown !== Mini.panelShown;
  Mini.layout = layout;
  Mini.panelShown = panelShown;
  if (changed) {
    Mini.lyricsFor = null;
    renderMiniPanel();
  }
}

function miniTime() {
  if (!Mini.on) return;
  const seek = $("#miniSeek");
  if (!seek || document.activeElement === seek) return;
  const a = Player.audio;
  const d = isFinite(a.duration) ? a.duration : 0;
  seek.value = d ? String(Math.round((a.currentTime / d) * 1000)) : "0";
  rangeFill(seek);
  $("#miniCur").textContent = fmtDur(a.currentTime * 1000);
  $("#miniDur").textContent = d ? fmtDur(d * 1000) : "0:00";
}

function renderMini() {
  if (!Mini.on) return;
  if (!$("#miniTitle")) return buildMini();
  updateMini();
  renderMiniPanel();
}

function bindMini() {
  miniLoad();
  $("#miniBtn")?.addEventListener("click", () => (Mini.on ? closeMini() : openMini()));
  window.addEventListener("player", (e) => {
    if (!Mini.on) return;
    const type = e.detail.type;
    if (type === "time") return miniTime();
    if (type === "volume" || type === "fx") return;
    updateMini();
    if (type === "queue" || type === "track") {
      if (Mini.prefs.panel === "queue" || type === "track") Mini.lyricsFor = null;
      renderMiniPanel();
    }
  });
  window.addEventListener("resize", () => {
    if (!Mini.on) return;
    miniLayout();
    clearTimeout(Mini.sizeTimer);
    Mini.sizeTimer = setTimeout(async () => {
      const win = miniWin();
      if (win && (await win.isMaximized().catch(() => false))) await win.unmaximize().catch(() => {});
      try {
        localStorage.setItem("atmo.miniSize", JSON.stringify({ w: innerWidth, h: innerHeight }));
      } catch {}
    }, 300);
  });
  document.addEventListener("keydown", (e) => {
    if (e.target.closest("input, textarea, select, [contenteditable]") || e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key.toLowerCase() === "p") {
      e.preventDefault();
      Mini.on ? closeMini() : openMini();
    } else if (e.key === "Escape" && Mini.on) closeMini();
  });
}
