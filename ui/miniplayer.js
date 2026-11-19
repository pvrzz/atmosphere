// atmosphere · miniplayer.js · Compact always-on-top player with optional cover, blurred backdrop and an editable queue
const Mini = {
  on: false,
  restore: null,
  prefs: { cover: true, blur: true, queue: false, pin: true },
  dragFrom: null,
};

const MINI_W = 340;
const MINI_BASE = 168;
const MINI_COVER = 300;
const MINI_QUEUE = 232;

function miniLoad() {
  try {
    Object.assign(Mini.prefs, JSON.parse(localStorage.getItem("atmo.mini") || "{}"));
  } catch {}
}

function miniSave() {
  try {
    localStorage.setItem("atmo.mini", JSON.stringify(Mini.prefs));
  } catch {}
}

const miniWin = () => window.__TAURI__?.window?.getCurrentWindow?.();

function miniHeight() {
  return MINI_BASE + (Mini.prefs.cover ? MINI_COVER : 0) + (Mini.prefs.queue ? MINI_QUEUE : 0);
}

async function miniResize() {
  const win = miniWin();
  const dpi = window.__TAURI__?.dpi;
  if (!win || !dpi) return;
  const min = MINI_BASE + (Mini.prefs.queue ? 120 : 0);
  await win.setMinSize(new dpi.LogicalSize(300, min)).catch(() => {});
  await win.setSize(new dpi.LogicalSize(MINI_W, miniHeight())).catch(() => {});
}

async function openMini() {
  if (Mini.on) return;
  if (typeof closeTv === "function" && Lyr?.tv) await closeTv();
  closeDrawers();
  closeFx();
  Mini.on = true;
  document.body.classList.add("mini-mode");
  $("#mini").hidden = false;
  $("#mini").classList.toggle("mac", isMac);
  const win = miniWin();
  if (win) {
    Mini.restore = { pos: await win.outerPosition().catch(() => null), size: await win.outerSize().catch(() => null), max: await win.isMaximized().catch(() => false) };
    if (Mini.restore.max) await win.unmaximize().catch(() => {});
    await miniResize();
    await win.setAlwaysOnTop(!!Mini.prefs.pin).catch(() => {});
  }
  renderMini();
}

async function closeMini() {
  if (!Mini.on) return;
  Mini.on = false;
  document.body.classList.remove("mini-mode");
  $("#mini").hidden = true;
  const win = miniWin();
  const dpi = window.__TAURI__?.dpi;
  const r = Mini.restore;
  Mini.restore = null;
  if (!win || !dpi) return;
  await win.setAlwaysOnTop(false).catch(() => {});
  await win.setMinSize(new dpi.LogicalSize(900, 620)).catch(() => {});
  if (r?.size) await win.setSize(new dpi.PhysicalSize(r.size.width, r.size.height)).catch(() => {});
  if (r?.pos) await win.setPosition(new dpi.PhysicalPosition(r.pos.x, r.pos.y)).catch(() => {});
  if (r?.max) await win.maximize().catch(() => {});
}

async function miniToggle(key) {
  Mini.prefs[key] = !Mini.prefs[key];
  miniSave();
  if (key === "pin") await miniWin()?.setAlwaysOnTop(Mini.prefs.pin).catch(() => {});
  if (key === "cover" || key === "queue") await miniResize();
  renderMini();
}

function miniButton(key, iconName, label) {
  const on = !!Mini.prefs[key];
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

function renderMini() {
  const box = $("#mini");
  if (!box || !Mini.on) return;
  const t = Player.current;
  const art = t?.art ? artUrl(t.art) : null;
  box.classList.toggle("with-cover", !!Mini.prefs.cover);
  box.classList.toggle("with-blur", !!Mini.prefs.blur && !!art);
  box.classList.toggle("with-queue", !!Mini.prefs.queue);
  box.style.setProperty("--mini-art", art ? `url("${art}")` : "none");
  const playing = !Player.audio.paused;
  const next = Player.queue.slice(Player.index + 1);
  const parts = [
    h("div", { class: "mini-bg", "aria-hidden": "true" }),
    h("div", { class: "mini-top", "data-tauri-drag-region": "" },
      h("span", { class: "mini-brand", "data-tauri-drag-region": "" }, "Atmosphere"),
      h("div", { class: "mini-opts" },
        miniButton("cover", "photo", Mini.prefs.cover ? "Hide cover art" : "Show cover art"),
        miniButton("blur", "droplet", Mini.prefs.blur ? "Turn off blurred backdrop" : "Blurred cover backdrop"),
        miniButton("queue", "list", Mini.prefs.queue ? "Hide queue" : "Show queue"),
        miniButton("pin", Mini.prefs.pin ? "pinned" : "pin", Mini.prefs.pin ? "Always on top: on" : "Always on top: off"),
        h("button", { class: "icon-btn", "aria-label": "Back to full window", title: "Back to full window (P)", onclick: closeMini }, icon("arrows-maximize")))),
    Mini.prefs.cover ? h("div", { class: "mini-art", "data-tauri-drag-region": "" }, art ? h("img", { src: art, alt: "", draggable: "false", "data-tauri-drag-region": "" }) : h("div", { class: "mini-art-empty" }, icon("music"))) : null,
    h("div", { class: "mini-meta" },
      h("span", { class: "mini-title", title: t?.title || "" }, t?.title || "Nothing playing"),
      h("span", { class: "mini-artist" }, t?.artist || "Pick something from your library")),
    h("div", { class: "mini-seek" },
      h("span", { class: "mini-t", id: "miniCur" }, "0:00"),
      h("input", { type: "range", class: "range", id: "miniSeek", min: "0", max: "1000", value: "0", "aria-label": "Seek" }),
      h("span", { class: "mini-t", id: "miniDur" }, "0:00")),
    h("div", { class: "mini-ctl" },
      h("button", { class: "icon-btn", "aria-label": "Previous", onclick: () => Player.prev() }, icon("player-skip-back-filled")),
      h("button", { class: "mini-play", "aria-label": playing ? "Pause" : "Play", onclick: () => Player.toggle() }, icon(playing ? "player-pause-filled" : "player-play-filled")),
      h("button", { class: "icon-btn", "aria-label": "Next", onclick: () => Player.next() }, icon("player-skip-forward-filled"))),
    Mini.prefs.queue
      ? h("div", { class: "mini-queue" },
          h("div", { class: "mq-head" }, h("span", {}, next.length ? `Up next · ${next.length}` : "Up next"), next.length ? h("button", { class: "btn small ghost", onclick: () => Player.clearUpNext() }, "Clear") : null),
          next.length ? h("div", { class: "mq-list" }, ...next.slice(0, 200).map((q, k) => miniQueueRow(q, Player.index + 1 + k))) : h("p", { class: "mq-empty" }, "Nothing queued. Add songs from your library."))
      : null,
  ];
  box.replaceChildren(...parts.filter(Boolean));
  const seek = $("#miniSeek");
  seek.addEventListener("input", () => {
    rangeFill(seek);
    const d = Player.audio.duration;
    if (isFinite(d)) $("#miniCur").textContent = fmtDur((seek.value / 1000) * d * 1000);
  });
  seek.addEventListener("change", () => Player.seek((seek.value / 1000) * Player.audio.duration));
  miniTime();
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

function bindMini() {
  miniLoad();
  $("#miniBtn")?.addEventListener("click", () => (Mini.on ? closeMini() : openMini()));
  window.addEventListener("player", (e) => {
    if (!Mini.on) return;
    const type = e.detail.type;
    if (type === "time") return miniTime();
    if (type === "volume" || type === "fx") return;
    renderMini();
  });
  document.addEventListener("keydown", (e) => {
    if (e.target.closest("input, textarea, select, [contenteditable]") || e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key.toLowerCase() === "p") {
      e.preventDefault();
      Mini.on ? closeMini() : openMini();
    } else if (e.key === "Escape" && Mini.on) closeMini();
  });
}
