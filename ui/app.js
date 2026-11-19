/* atmosphere · app.js · Frontend logic: onboarding, command bar, release sheets, metadata editing, save options, downloads and settings. */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const svg = (name) => (window.ICONS[name] ? `<svg viewBox="0 0 24 24" aria-hidden="true">${window.ICONS[name]}</svg>` : "");
const reduced = matchMedia("(prefers-reduced-motion: reduce)");

function h(tag, attrs = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") el.className = v;
    else if (k === "icon") el.innerHTML = svg(v);
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) if (kid != null && kid !== false) el.append(kid.nodeType ? kid : String(kid));
  return el;
}
const icon = (name) => h("span", { class: "i", icon: name });
const isMac = /Mac|iPhone|iPad/.test(navigator.userAgentData?.platform || navigator.platform || navigator.userAgent);
const platformKeys = () => isMac && $$("kbd").forEach((k) => (k.textContent = k.textContent.replace(/^Ctrl /, "⌘ ")));
const hydrate = (root = document) => $$("[data-icon]", root).forEach((el) => (el.innerHTML = svg(el.dataset.icon)));

const KIND = { album: "Album", ep: "EP", single: "Single", lp: "LP", compilation: "Compilation", mixtape: "Mixtape", playlist: "Playlist", profile: "Profile", channel: "Channel", search: "Search results", track: "Track" };
const RELEASES = new Set(["album", "ep", "single", "lp", "compilation", "mixtape"]);
const SOURCE = { title: "Detected from the title", metadata: "Detected from track details", platform: "As listed on the platform", you: "Set by you" };
const FINAL = new Set(["done", "skipped", "error", "cancelled"]);
const ACTIVE = new Set(["queued", "preparing", "downloading", "processing", "tagging"]);
const PLATFORM_ICON = [
  [/^\S+\.(mp3|wav|flac|m4a|aac|ogg|oga|opus|aiff?|wma|alac|mp4|m4v|mov|webm|mkv)(?:[?#]\S*)?$/i, "file-music", "File link"],
  [/soundcloud\.com|snd\.sc/i, "brand-soundcloud", "SoundCloud"],
  [/youtube\.com|youtu\.be|music\.youtube/i, "brand-youtube", "YouTube"],
  [/bandcamp\.com/i, "brand-bandcamp", "Bandcamp"],
  [/vimeo\.com/i, "brand-vimeo", "Vimeo"],
  [/twitch\.tv/i, "brand-twitch", "Twitch"],
  [/tiktok\.com/i, "brand-tiktok", "TikTok"],
  [/(^|\.)x\.com|twitter\.com/i, "brand-x", "X"],
  [/instagram\.com/i, "brand-instagram", "Instagram"],
];
const codecName = (c) => ({ opus: "Opus", vorbis: "Vorbis", mp4a: "AAC", aac: "AAC", pcm: "PCM" })[(c || "").toLowerCase()] || (c || "").toUpperCase();
const platformIcon = (name) => (PLATFORM_ICON.find((p) => p[2] === name) || [0, "world"])[1];

const state = {
  settings: null,
  tools: null,
  view: "get",
  collections: [],
  pending: [],
  active: null,
  scope: "youtube",
  jobs: new Map(),
  groups: [],
  early: new Map(),
  history: [],
  historyDirty: true,
  clipDismissed: new Set(),
  installing: {},
};

/* formatting */
const nf = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 });
function fmtDur(ms) {
  if (!ms && ms !== 0) return "";
  const s = Math.round(ms / 1000), hh = Math.floor(s / 3600), mm = Math.floor((s % 3600) / 60), ss = String(s % 60).padStart(2, "0");
  return hh ? `${hh}:${String(mm).padStart(2, "0")}:${ss}` : `${mm}:${ss}`;
}
function fmtLong(ms) {
  const m = Math.round(ms / 60000);
  return m >= 60 ? `${Math.floor(m / 60)} hr ${m % 60} min` : `${m} min`;
}
function fmtBytes(n) {
  if (!n) return "0 MB";
  const u = ["B", "KB", "MB", "GB"], i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), 3);
  return `${nf.format(n / 1024 ** i)} ${u[i]}`;
}
const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "short", day: "numeric" });
function dayLabel(ts) {
  const d = new Date(ts), today = new Date();
  const diff = Math.round((new Date(today.toDateString()) - new Date(d.toDateString())) / 86400000);
  return diff === 0 ? "Today" : diff === 1 ? "Yesterday" : dayFmt.format(d);
}
const plural = (n, one, many = one + "s") => `${n} ${n === 1 ? one : many}`;
const errText = (e) => (typeof e === "string" ? e : e?.message || "Something went wrong.");

function toast(message, kind = "") {
  const t = h("div", { class: `toast ${kind}`, role: "status" }, icon(kind === "err" ? "alert-circle" : "circle-check"), h("span", {}, message));
  $("#toasts").append(t);
  setTimeout(() => {
    t.classList.add("out");
    setTimeout(() => t.remove(), 220);
  }, kind === "err" ? 6500 : 3000);
}

function art(url, cls, alt = "") {
  if (!url) return h("div", { class: `${cls} ph` }, icon("music"));
  const img = h("img", { class: cls, src: url, alt, loading: "lazy", draggable: "false", referrerpolicy: "no-referrer" });
  img.addEventListener("error", () => img.replaceWith(h("div", { class: `${cls} ph` }, icon("music"))), { once: true });
  return img;
}

/* theme */
function applyTheme() {
  const t = state.settings.theme;
  if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
  else delete document.documentElement.dataset.theme;
  syncFrame();
}

function syncFrame() {
  const color = getComputedStyle(document.documentElement).getPropertyValue("--border").trim();
  if (/^#[0-9a-f]{6}$/i.test(color)) invoke("frame_color", { color }).catch(() => {});
}

/* window chrome */
const SIDE_KEY = "atmo.sidebar";

function setSidebar(collapsed, save = true) {
  document.documentElement.classList.toggle("side-collapsed", collapsed);
  const b = $("#sideToggle");
  if (b) {
    b.setAttribute("aria-expanded", String(!collapsed));
    b.setAttribute("aria-label", collapsed ? "Expand sidebar" : "Collapse sidebar");
    b.dataset.tip = collapsed ? "Expand sidebar (Ctrl+B)" : "Collapse sidebar (Ctrl+B)";
    b.replaceChildren(icon(collapsed ? "layout-sidebar-left-expand" : "layout-sidebar-left-collapse"));
  }
  hideSideTip();
  if (save) {
    try {
      localStorage.setItem(SIDE_KEY, collapsed ? "collapsed" : "open");
    } catch {}
  }
}

function sideTipText(el) {
  if (el.dataset.tip) return el.dataset.tip;
  const label = el.querySelector(".nav-label") || [...el.children].find((c) => c.tagName === "SPAN" && !c.classList.contains("i") && !c.classList.contains("nav-ring") && !c.classList.contains("count") && c.textContent.trim());
  const meta = el.querySelector(".nav-meta")?.textContent.trim();
  const count = el.querySelector(".count:not([hidden])")?.textContent.trim();
  const base = (label?.textContent || el.getAttribute("aria-label") || "").trim();
  return [base, meta === "Missing" ? "folder missing" : meta && el.closest("#playlistNav") ? `${meta} songs` : "", count ? `${count} active` : ""].filter(Boolean).join(" · ");
}

function showSideTip(el) {
  const text = sideTipText(el);
  if (!text) return hideSideTip();
  let tip = $("#sideTip");
  if (!tip) {
    tip = h("div", { class: "side-tip", id: "sideTip", role: "tooltip" });
    document.body.append(tip);
  }
  tip.textContent = text;
  const r = el.getBoundingClientRect();
  tip.style.top = `${Math.round(r.top + r.height / 2)}px`;
  tip.style.left = `${Math.round(r.right + 10)}px`;
  tip.classList.add("on");
}

function hideSideTip() {
  $("#sideTip")?.classList.remove("on");
}

function bindSidebar() {
  let collapsed = false;
  try {
    collapsed = localStorage.getItem(SIDE_KEY) === "collapsed";
  } catch {}
  setSidebar(collapsed, false);
  const side = $("#sidebar");
  $("#sideToggle").addEventListener("click", () => setSidebar(!document.documentElement.classList.contains("side-collapsed")));
  const target = (e) => e.target.closest?.(".nav-item, .update-chip, .engine-state, .side-toggle, #newPlaylistBtn");
  side.addEventListener("mouseover", (e) => {
    const el = target(e);
    if (el && document.documentElement.classList.contains("side-collapsed")) showSideTip(el);
    else hideSideTip();
  });
  side.addEventListener("mouseleave", hideSideTip);
  side.addEventListener("focusin", (e) => {
    const el = target(e);
    if (el && el.matches(":focus-visible") && document.documentElement.classList.contains("side-collapsed")) showSideTip(el);
  });
  side.addEventListener("focusout", hideSideTip);
  side.addEventListener("click", hideSideTip);
  $("#playlistNav")?.addEventListener("scroll", hideSideTip, { passive: true });
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "b" && !e.target.closest("input, textarea, [contenteditable]")) {
      e.preventDefault();
      setSidebar(!document.documentElement.classList.contains("side-collapsed"));
    }
  });
}

function bindWindow() {
  const bar = $("#titlebar");
  document.documentElement.classList.toggle("mac", isMac);
  if (isMac) bar.classList.add("mac");
  const api = window.__TAURI__.window;
  if (!api?.getCurrentWindow) return;
  const win = api.getCurrentWindow();
  const restoreSvg = '<svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2.5 2.5V0.5h7v7h-2" /><rect x="0.5" y="2.5" width="7" height="7" /></svg>';
  const maxSvg = '<svg viewBox="0 0 10 10" aria-hidden="true"><rect x="0.5" y="0.5" width="9" height="9" /></svg>';
  const syncMax = async () => {
    const maxed = await win.isMaximized().catch(() => false);
    const b = $("#winMax");
    b.innerHTML = maxed ? restoreSvg : maxSvg;
    b.setAttribute("aria-label", maxed ? "Restore" : "Maximize");
    b.title = maxed ? "Restore" : "Maximize";
  };
  $("#winMin").addEventListener("click", () => win.minimize());
  $("#winMax").addEventListener("click", () => win.toggleMaximize());
  $("#winClose").addEventListener("click", () => win.close());
  win.onResized?.(() => syncMax());
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", syncFrame);
  window.addEventListener("focus", () => bar.classList.remove("blur"));
  window.addEventListener("blur", () => bar.classList.add("blur"));
  syncMax();
}

/* boot */
async function init() {
  hydrate();
  platformKeys();
  bindWindow();
  bindSidebar();
  state.settings = await invoke("get_settings");
  applyTheme();
  bindShell();
  bindCommand();
  bindCommit();
  bindSettings();
  bindPlayer();
  bindLibrary();
  bindLyrics();
  bindBackup();
  bindMini();
  bindConnect();
  bindUpdates();
  bindPlaybackSettings();
  bindSearch();
  bindTools();
  bindStats();
  await listen("atmosphere://job", (e) => onJob(e.payload));
  await listen("atmosphere://probe", (e) => onProbe(e.payload));
  await listen("atmosphere://install", (e) => onInstall(e.payload));
  await refreshTools();
  if (!state.settings.rightsAccepted) showOnboarding(1);
  else unlock();
  const cleaned = await invoke("startup_cleanup").catch(() => null);
  if (cleaned) toast(`Cleaned up ${plural(cleaned.files, "leftover file")} from an earlier session (${fmtBytes(cleaned.bytes)})`, "ok");
}

function unlock() {
  $("#onboarding").hidden = true;
  $("#shell").removeAttribute("inert");
  renderAll();
  $("#commandInput").focus();
  checkClipboard();
}

/* onboarding */
function showOnboarding(step) {
  $("#shell").setAttribute("inert", "");
  $("#onboarding").hidden = false;
  $("#obStep1").hidden = step !== 1;
  $("#obStep2").hidden = step !== 2;
  $("#obStep1Dot").className = step === 1 ? "on" : "done";
  $("#obStep2Dot").className = step === 2 ? "on" : "";
  if (step === 2) renderObTools();
}

function bindOnboarding() {
  const checks = $$(".pledge-check");
  checks.forEach((c) => c.addEventListener("change", () => ($("#obAccept").disabled = !checks.every((x) => x.checked))));
  $("#obAccept").addEventListener("click", async () => {
    try {
      state.settings = await invoke("accept_rights", { version: state.settings.termsVersion });
      if (!state.tools?.ytdlp.path || !state.tools?.ffmpeg.path) showOnboarding(2);
      else unlock();
    } catch (e) {
      toast(errText(e), "err");
    }
  });
  $("#obSkip").addEventListener("click", unlock);
  $("#obInstall").addEventListener("click", async () => {
    const btn = $("#obInstall");
    btn.disabled = true;
    const ok = await installMissing();
    btn.disabled = false;
    if (ok) unlock();
  });
}

function toolRow(key, tool, name, blurb) {
  const busy = state.installing[key];
  const ready = !!tool.path;
  const row = h("div", { class: "ob-tool" },
    h("div", {}, h("strong", {}, name), h("span", {}, ready ? `Ready${tool.version ? ` · ${tool.version}` : ""}` : blurb)),
    ready ? h("span", { class: "flag" }, "Installed") : busy ? h("span", { class: "flag" }, "Downloading…") : h("span", { class: "flag warn" }, "Not installed")
  );
  if (busy) row.append(progressBar(busy.total ? busy.received / busy.total : null));
  return row;
}

function renderObTools() {
  const t = state.tools;
  if (!t) return;
  $("#obTools").replaceChildren(
    toolRow("ytdlp", t.ytdlp, "yt-dlp", "Reads links and downloads media (about 18 MB)."),
    toolRow("ffmpeg", t.ffmpeg, "ffmpeg", t.canInstallFfmpeg ? "Converts formats and writes tags (about 90 MB)." : isMac ? "Run brew install ffmpeg in Terminal; Atmosphere finds it." : "Install it with your package manager.")
  );
  $("#obInstall").textContent = t.ytdlp.path && t.ffmpeg.path ? "Continue" : "Set up both";
}

async function installTool(key) {
  state.installing[key] = { received: 0, total: null };
  renderEngine();
  try {
    state.tools = await invoke("install_tool", { tool: key });
    return true;
  } catch (e) {
    toast(errText(e), "err");
    return false;
  } finally {
    delete state.installing[key];
    renderEngine();
  }
}

async function installMissing() {
  let ok = true;
  if (!state.tools.ytdlp.path) ok = (await installTool("ytdlp")) && ok;
  if (!state.tools.ffmpeg.path && state.tools.canInstallFfmpeg) ok = (await installTool("ffmpeg")) && ok;
  return ok;
}

function onInstall(p) {
  if (!state.installing[p.tool]) return;
  state.installing[p.tool] = p;
  renderEngine();
}

async function refreshTools() {
  try {
    state.tools = await invoke("tools_status");
  } catch {
    state.tools = null;
  }
  renderEngine();
}

function renderEngine() {
  const t = state.tools;
  const foot = $("#engineState");
  const ready = !!t?.ytdlp.path;
  const engineText = ready ? (t.ffmpeg.path ? "Engine ready" : "ffmpeg missing: limited formats") : "Engine not set up";
  foot.dataset.tip = engineText;
  foot.replaceChildren(h("span", { class: `dot ${ready ? (t.ffmpeg.path ? "ok" : "warn") : "warn"}` }), h("span", {}, engineText));
  $("#engineBanner").hidden = ready || !state.settings?.rightsAccepted;
  if (!$("#onboarding").hidden) renderObTools();
  renderToolSettings();
  updateCommit();
}

function progressBar(fraction, cls = "") {
  const bar = h("div", { class: `bar ${cls} ${fraction == null ? "indet" : ""}` }, h("i"));
  if (fraction != null) bar.firstChild.style.transform = `scaleX(${Math.max(0, Math.min(1, fraction))})`;
  return bar;
}

/* shell & navigation */
function bindShell() {
  bindOnboarding();
  $$("[data-nav]").forEach((b) =>
    b.addEventListener("click", () => {
      go(b.dataset.nav);
      if (b.dataset.anchor) $(`#set-${b.dataset.anchor}`)?.scrollIntoView({ block: "start" });
    })
  );
  $("#clearFetched").addEventListener("click", () => {
    state.collections.forEach((c) => invoke("forget", { id: c.data.id }));
    state.collections = [];
    state.active = null;
    renderAll();
  });
  $("#engineBannerBtn").addEventListener("click", async () => {
    const b = $("#engineBannerBtn");
    b.disabled = true;
    await installMissing();
    b.disabled = false;
  });
  window.addEventListener("focus", checkClipboard);
  document.addEventListener("keydown", onKey);
  document.addEventListener("paste", (e) => {
    if (e.target.closest("input, textarea, [contenteditable]")) return;
    const text = e.clipboardData.getData("text").trim();
    if (/^https?:\/\//i.test(text)) {
      e.preventDefault();
      go("get");
      fetchText(text);
    }
  });
  document.addEventListener("dragstart", (e) => {
    const t = e.target;
    if (t.nodeType !== 1 || (t.getAttribute("draggable") !== "true" && !t.closest?.('[draggable="true"]'))) e.preventDefault();
  });
  document.addEventListener("dragover", (e) => e.preventDefault());
  document.addEventListener("drop", (e) => {
    e.preventDefault();
    const text = (e.dataTransfer.getData("text/uri-list") || e.dataTransfer.getData("text/plain") || "").trim();
    if (/^https?:\/\//i.test(text)) {
      go("get");
      fetchText(text);
    }
  });
  document.addEventListener("click", (e) => {
    if (!e.target.isConnected) return;
    if (!e.target.closest("#saveMenu, #saveAsBtn")) closeSaveMenu();
    if (!e.target.closest("#typeMenu, .kind-btn")) $("#typeMenu").hidden = true;
  });
}

function go(view) {
  $$(".nav-item[data-nav]").forEach((b) => b.dataset.nav === view ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current"));
  if (state.view === view) return markLibNav();
  state.view = view;
  $$(".view").forEach((v) => (v.hidden = v.dataset.view !== view));
  closeSaveMenu();
  closeMenus();
  if (view !== "library") closeDrawers();
  if (view === "downloads") renderDownloads(true);
  if (view === "settings") renderSettings();
  markLibNav();
}

function onKey(e) {
  const mod = e.ctrlKey || e.metaKey;
  if (mod && e.key.toLowerCase() === "k") {
    e.preventDefault();
    openSearch();
  } else if (mod && e.key.toLowerCase() === "l") {
    e.preventDefault();
    go("get");
    $("#commandInput").focus();
    $("#commandInput").select();
  } else if (e.key === "Escape") {
    if (!$("#saveMenu").hidden) closeSaveMenu();
    else if (!$("#typeMenu").hidden) $("#typeMenu").hidden = true;
    else if (current()?.inspected != null && !current().data.single) {
      current().inspected = null;
      renderSheet();
    }
  } else if (mod && e.key === "Enter" && state.view === "get") {
    startDownload();
  }
}

function renderAll() {
  renderFetched();
  renderSheet();
  renderEngine();
  updateNav();
}

/* clipboard */
async function checkClipboard() {
  if (!state.settings?.rightsAccepted) return;
  const url = await invoke("read_clipboard").catch(() => null);
  const known = state.collections.some((c) => c.data.url === url || c.source === url);
  const show = url && !state.clipDismissed.has(url) && !known && !$("#commandInput").value.trim();
  $("#clipChip").hidden = !show;
  if (show) $("#clipUrl").textContent = url.replace(/^https?:\/\/(www\.)?/, "");
  $("#clipChip").dataset.url = url || "";
}

/* command bar */
function classifyInput(text) {
  const t = text.trim();
  if (!t) return { kind: "empty" };
  const urls = t.split(/\s+/).filter((x) => /^(https?:\/\/|www\.)\S+$/i.test(x) || /^[\w-]+(\.[\w-]+)+\/\S*$/.test(x));
  if (urls.length && urls.length === t.split(/\s+/).length) {
    const hit = PLATFORM_ICON.find(([re]) => re.test(urls[0]));
    return { kind: "url", urls, icon: hit ? hit[1] : "world", platform: hit ? hit[2] : null };
  }
  return { kind: "query", query: t };
}

function bindCommand() {
  const input = $("#commandInput");
  const sync = () => {
    const c = classifyInput(input.value);
    $("#commandGlyph").innerHTML = svg(c.kind === "url" ? c.icon : c.kind === "query" ? "search" : "link");
    $("#commandGlyph").classList.toggle("known", c.kind === "url" && !!c.platform);
    $("#searchScope").hidden = c.kind !== "query";
    $("#commandGo .label").textContent = c.kind === "query" ? "Search" : "Fetch";
    if (input.value.trim()) $("#clipChip").hidden = true;
  };
  input.addEventListener("input", sync);
  $$("#searchScope button").forEach((b) =>
    b.addEventListener("click", () => {
      state.scope = b.dataset.scope;
      $$("#searchScope button").forEach((x) => x.setAttribute("aria-checked", String(x === b)));
      input.focus();
    })
  );
  $("#commandForm").addEventListener("submit", (e) => {
    e.preventDefault();
    fetchText(input.value);
  });
  $("#clipUse").addEventListener("click", () => {
    const url = $("#clipChip").dataset.url;
    $("#clipChip").hidden = true;
    fetchText(url);
  });
  $("#clipDismiss").addEventListener("click", () => {
    state.clipDismissed.add($("#clipChip").dataset.url);
    $("#clipChip").hidden = true;
  });
  sync();
}

async function fetchText(text) {
  const c = classifyInput(text);
  if (c.kind === "empty") return $("#commandInput").focus();
  if (!state.tools?.ytdlp.path) return toast("Set up the engine first. It reads links for Atmosphere.", "err");
  const btn = $("#commandGo");
  btn.classList.add("loading");
  $("#clipChip").hidden = true;
  const tasks = c.kind === "url" ? c.urls.map((u) => ({ url: u, label: u.replace(/^https?:\/\/(www\.)?/, "") })) : [{ query: c.query, label: `“${c.query}”` }];
  const pend = tasks.map((t) => ({ key: Math.random().toString(36).slice(2), label: t.label }));
  state.pending.push(...pend);
  renderFetched();
  $("#commandInput").value = "";
  $("#commandInput").dispatchEvent(new Event("input"));
  await Promise.all(
    tasks.map(async (t, i) => {
      try {
        const data = t.url ? await invoke("probe", { url: t.url }) : await invoke("search", { platform: state.scope, query: t.query });
        addCollection(data, t.url || t.query);
      } catch (e) {
        toast(`${tasks.length > 1 ? t.label + ": " : ""}${errText(e)}`, "err");
      } finally {
        state.pending = state.pending.filter((p) => p !== pend[i]);
        renderFetched();
      }
    })
  );
  btn.classList.remove("loading");
}

/* collections */
function selectable(e) {
  return !["drm", "preview"].includes(e.availability);
}

function addCollection(data, source) {
  const existing = state.collections.findIndex((c) => c.data.url && c.data.url === data.url);
  if (existing >= 0) {
    invoke("forget", { id: state.collections[existing].data.id });
    state.collections.splice(existing, 1);
  }
  const c = {
    data,
    source,
    releaseType: data.releaseType,
    releaseSource: data.releaseTypeSource,
    selected: new Set(data.releaseType === "search" ? [] : data.entries.filter(selectable).map((e) => e.index)),
    album: { title: data.title, artist: data.artist, date: data.year ? String(data.year) : "", genre: "", label: "", artwork: { kind: "keep" }, preview: null },
    albumEdited: new Set(),
    overrides: new Map(),
    trims: new Map(),
    inspected: data.single ? data.entries[0]?.index : null,
  };
  state.collections.unshift(c);
  state.active = c;
  $("#attest").checked = false;
  go("get");
  renderAll();
}

const current = () => state.active;
const entryOf = (c, index) => c.data.entries.find((e) => e.index === index);
const isRelease = (c) => RELEASES.has(c.releaseType);
const numbered = (c) => !c.data.single && (isRelease(c) || c.releaseType === "playlist");

function position(c, entry) {
  return c.data.entries.indexOf(entry) + 1;
}

function defaults(c, e) {
  const rel = isRelease(c);
  const n = numbered(c);
  return {
    title: e.title || "",
    artist: e.artist || "",
    album: rel ? c.album.title : e.album || (c.releaseType === "playlist" ? c.album.title : ""),
    albumArtist: rel ? c.album.artist : e.albumArtist || "",
    genre: (rel && c.album.genre) || e.genre || "",
    date: (rel && c.album.date) || e.releaseDate || (e.year ? String(e.year) : ""),
    trackNumber: n ? position(c, e) : e.trackNumber ?? null,
    trackTotal: n ? c.data.entries.length : null,
    discNumber: e.discNumber ?? null,
    composer: e.composer || "",
    label: (rel && c.album.label) || "",
    isrc: "",
    bpm: null,
    key: "",
    comment: [e.description?.trim(), e.url ? `Source: ${e.url}` : ""].filter(Boolean).join("\n\n"),
    lyrics: e.lyrics || "",
    artwork: rel || c.data.single ? c.album.artwork : { kind: "keep" },
  };
}

function finalMeta(c, e) {
  return { ...defaults(c, e), ...(c.overrides.get(e.index) || {}) };
}

function folderFor(c) {
  if (c.data.single || c.releaseType === "search") return null;
  if (isRelease(c)) return [c.album.artist, c.album.title].filter(Boolean).join(" - ");
  return c.album.title;
}

function selectedEntries(c) {
  return c.data.entries.filter((e) => c.selected.has(e.index));
}

/* probe events */
function onProbe(p) {
  const c = state.collections.find((x) => x.data.id === p.collection);
  if (!c) return;
  if (p.type === "entry") {
    const i = c.data.entries.findIndex((e) => e.index === p.entry.index);
    if (i >= 0) {
      const wasSelectable = selectable(c.data.entries[i]);
      c.data.entries[i] = p.entry;
      if (wasSelectable && !selectable(p.entry)) c.selected.delete(p.entry.index);
    }
    if (c === state.active) scheduleSheet();
  } else if (p.type === "done") {
    c.data.enriching = false;
    if (c.releaseSource !== "you") {
      c.releaseType = p.releaseType;
      c.releaseSource = p.releaseTypeSource;
    }
    if (!c.albumEdited.has("artist") && p.artist) c.album.artist = p.artist;
    if (!c.albumEdited.has("date")) {
      const dates = [...new Set(c.data.entries.map((e) => e.releaseDate).filter(Boolean))];
      if (dates.length === 1) c.album.date = dates[0];
      else if (p.year) c.album.date = String(p.year);
    }
    if (p.failed) c.unreadable = p.failed;
    renderFetched();
    if (c === state.active) renderSheet();
  }
}

let sheetQueued = false;
function scheduleSheet() {
  if (sheetQueued) return;
  sheetQueued = true;
  requestAnimationFrame(() => {
    sheetQueued = false;
    renderSheet(true);
  });
}

/* sidebar list */
function renderFetched() {
  const list = $("#fetchedList");
  const items = [
    ...state.pending.map((p) =>
      h("li", {}, h("div", { class: "fetched-item" }, h("div", { class: "thumb ph" }, h("span", { class: "spin-sm" }, icon("loader-2"))), h("div", {}, h("div", { class: "t" }, "Reading link…"), h("div", { class: "m" }, p.label))))
    ),
    ...state.collections.map((c) => {
      const n = c.data.entries.length;
      const meta = c.data.single ? `${c.data.entries[0]?.platform || ""}${c.data.entries[0]?.durationMs ? " · " + fmtDur(c.data.entries[0].durationMs) : ""}` : `${KIND[c.releaseType] || "Playlist"} · ${plural(n, "item")}`;
      return h("li", {}, h("button", { class: "fetched-item", "data-cid": c.data.id, "aria-current": String(c === state.active), onclick: () => { state.active = c; go("get"); renderFetched(); renderSheet(); } },
        art(c.data.thumbnail, "thumb"), h("div", { style: "min-width:0" }, h("div", { class: "t" }, c.album.title || c.data.title), h("div", { class: "m" }, meta))));
    }),
  ];
  list.replaceChildren(...items);
  $("#fetchedStrip").hidden = items.length === 0;
  $("#clearFetched").hidden = state.collections.length === 0;
}

/* release sheet */
function editable(text, placeholder, onCommit, cls = "") {
  const el = h("span", { class: `editable ${cls}`, contenteditable: "plaintext-only", spellcheck: "false", role: "textbox", "aria-label": placeholder, "data-placeholder": placeholder }, text || "");
  el.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      el.blur();
    }
    if (e.key === "Escape") {
      el.textContent = text || "";
      el.blur();
    }
  });
  el.addEventListener("blur", () => {
    const v = el.textContent.trim();
    if (v !== (text || "")) onCommit(v);
  });
  return el;
}

function setAlbum(c, key, value) {
  c.album[key] = value;
  c.albumEdited.add(key);
  renderFetched();
  renderSheet();
}

function setOverride(c, index, key, value) {
  const e = entryOf(c, index);
  const base = defaults(c, e)[key];
  const o = { ...(c.overrides.get(index) || {}) };
  const same = (base ?? "") === (value ?? "") || (base == null && value === "");
  if (same) delete o[key];
  else o[key] = value;
  Object.keys(o).length ? c.overrides.set(index, o) : c.overrides.delete(index);
}

function renderSheet(soft = false) {
  const c = current();
  const sheet = $("#sheet");
  $("#emptyGet").hidden = !!c;
  sheet.hidden = !c;
  $("#commit").hidden = !c;
  if (!c) return;
  const scrollY = $("#getScroll").scrollTop;
  sheet.replaceChildren(sheetHead(c), sheetBody(c));
  if (soft) $("#getScroll").scrollTop = scrollY;
  updateCommit();
}

function sheetHead(c) {
  const d = c.data;
  const single = d.single;
  const e0 = d.entries[0];
  const cover = c.album.preview || d.thumbnail;
  const total = d.entries.reduce((a, e) => a + (e.durationMs || 0), 0);
  const coverEl = c.album.artwork.kind === "none" ? h("div", { class: "cover ph" }, icon("photo")) : art(cover, "cover", "Cover art");
  coverEl.id = "sheetCover";
  const canEditCover = single || isRelease(c);
  const coverWrap = h("div", { class: "cover-wrap" }, coverEl, canEditCover && h("button", { class: "cover-edit", onclick: () => chooseCover(c) }, icon("photo"), "Change"));

  const kindLine = h("div", { class: "kind-line" });
  if (single) {
    kindLine.append(h("span", { class: "kind-btn", style: "cursor:default" }, icon(platformIcon(e0.platform)), e0.platform || "Media"));
    if (e0.media?.hasVideo) kindLine.append(h("span", { class: "sep" }, "·"), "Video");
    if (e0.year) kindLine.append(h("span", { class: "sep" }, "·"), String(e0.year));
  } else if (c.releaseType === "search") {
    kindLine.append(h("span", { class: "kind-btn", style: "cursor:default" }, icon("search"), `Search · ${state.scope === "soundcloud" ? "SoundCloud" : "YouTube"}`));
  } else {
    const kb = h("button", { class: "kind-btn", "aria-haspopup": "menu", title: SOURCE[c.releaseSource] || "Change type", onclick: (ev) => openTypeMenu(ev.currentTarget, c) }, KIND[c.releaseType] || "Playlist", icon("chevron-down"));
    kindLine.append(kb);
    if (SOURCE[c.releaseSource]) kindLine.append(h("span", { class: "kind-src" }, SOURCE[c.releaseSource].replace("Detected from the", "from").replace("Detected from", "from").replace("As listed on the platform", "per platform").replace("Set by you", "set by you")));
    kindLine.append(h("span", { class: "sep" }, "·"), h("span", {}, d.platform));
  }

  let title, artist;
  if (single) {
    const m = finalMeta(c, e0);
    title = editable(m.title, "Title", (v) => { setOverride(c, e0.index, "title", v); renderFetched(); renderSheet(); }, "head-title");
    artist = editable(m.artist, "Artist", (v) => { setOverride(c, e0.index, "artist", v); renderSheet(); }, "head-artist");
  } else if (c.releaseType === "search") {
    title = h("div", { class: "head-title" }, `“${d.title}”`);
    artist = h("div", { class: "head-artist", style: "color:var(--text-2)" }, `${plural(d.entries.length, "result")}. Tick what you want to keep.`);
  } else {
    title = editable(c.album.title, isRelease(c) ? "Album title" : "Playlist name", (v) => setAlbum(c, "title", v), "head-title");
    artist = editable(c.album.artist, isRelease(c) ? "Album artist" : "Curator", (v) => setAlbum(c, "artist", v), "head-artist");
  }

  const facts = h("div", { class: "head-facts" });
  if (!single && c.releaseType !== "search") {
    facts.append(h("span", {}, `${plural(d.entries.length, isRelease(c) ? "track" : "item")}${total ? " · " + fmtLong(total) : ""}`));
    if (isRelease(c)) {
      facts.append(
        h("span", { class: "fact" }, h("b", {}, "Year"), editable(c.album.date, "Add", (v) => setAlbum(c, "date", v))),
        h("span", { class: "fact" }, h("b", {}, "Genre"), editable(c.album.genre, "Add", (v) => setAlbum(c, "genre", v))),
        h("span", { class: "fact" }, h("b", {}, "Label"), editable(c.album.label, "Add", (v) => setAlbum(c, "label", v)))
      );
    }
  } else if (single) {
    facts.append(h("span", {}, [e0.durationMs ? fmtDur(e0.durationMs) : null, e0.uploader && e0.uploader !== finalMeta(c, e0).artist ? `uploaded by ${e0.uploader}` : null, e0.license ? licenceLabel(e0.license) : null].filter(Boolean).join(" · ")));
  }

  const actions = h("div", { class: "head-actions" },
    d.url && h("button", { class: "btn small", onclick: () => invoke("open_link", { url: d.url }) }, icon("external-link"), `Open on ${d.platform || "site"}`),
    h("button", { class: "btn small ghost", onclick: () => removeCollection(c) }, "Remove")
  );

  return h("header", { class: "sheet-head" },
    h("div", { class: "sheet-art-bg", style: cover && c.album.artwork.kind !== "none" ? `background-image:url("${cover.replace(/"/g, "%22")}")` : "" }),
    coverWrap,
    h("div", { class: "head-meta" }, kindLine, title, artist, facts, actions)
  );
}

function licenceLabel(l) {
  const map = { "all-rights-reserved": "All rights reserved", "no-rights-reserved": "Public domain", "cc-by": "CC BY", "cc-by-sa": "CC BY-SA", "cc-by-nd": "CC BY-ND", "cc-by-nc": "CC BY-NC", "cc-by-nc-sa": "CC BY-NC-SA", "cc-by-nc-nd": "CC BY-NC-ND" };
  return map[l] || l;
}

function removeCollection(c) {
  invoke("forget", { id: c.data.id });
  state.collections = state.collections.filter((x) => x !== c);
  state.active = state.collections[0] || null;
  renderAll();
}

async function chooseCover(c) {
  try {
    const picked = await invoke("pick_image");
    if (!picked) return;
    if (c.data.single) {
      setOverride(c, c.data.entries[0].index, "artwork", { kind: "file", path: picked.path });
    }
    c.album.artwork = { kind: "file", path: picked.path };
    c.album.preview = picked.dataUrl;
    renderSheet();
  } catch (e) {
    toast(errText(e), "err");
  }
}

function openTypeMenu(anchor, c) {
  const menu = $("#typeMenu");
  menu.replaceChildren(
    ...["album", "ep", "single", "lp", "compilation", "mixtape", "playlist"].map((k) =>
      h("button", { role: "menuitemradio", "aria-checked": String(c.releaseType === k), onclick: () => { c.releaseType = k; c.releaseSource = "you"; menu.hidden = true; renderFetched(); renderSheet(); } }, KIND[k])
    ),
    h("div", { class: "menu-note" }, "Albums, EPs and singles get album tags and track numbers.")
  );
  const r = anchor.getBoundingClientRect();
  menu.style.left = `${r.left}px`;
  menu.style.top = `${r.bottom + 6}px`;
  menu.hidden = false;
  menu.querySelector("button")?.focus();
}

function sheetBody(c) {
  const d = c.data;
  if (d.single) return h("div", { class: "sheet-body" }, inspector(c, d.entries[0], true));
  const body = h("div", { class: `sheet-body ${c.inspected != null ? "inspecting" : ""}` });
  const sel = c.selected.size;
  const avail = d.entries.filter(selectable).length;
  const allBox = h("input", { type: "checkbox", class: "tl-check", "aria-label": "Select all" });
  allBox.checked = sel > 0 && sel === avail;
  allBox.indeterminate = sel > 0 && sel < avail;
  allBox.addEventListener("change", () => {
    c.selected = new Set(allBox.checked ? d.entries.filter(selectable).map((e) => e.index) : []);
    renderSheet(true);
  });
  const detailed = d.entries.filter((e) => e.detailed).length;
  const unavailable = d.entries.length - avail;
  body.append(
    h("div", { class: "tl-tools" },
      allBox,
      h("span", {}, `${sel} of ${d.entries.length} selected`),
      h("span", { class: "grow" }),
      d.enriching && h("span", {}, `Reading details… ${detailed} of ${d.entries.length}`),
      !d.enriching && unavailable > 0 && unavailableNote(d.entries)
    )
  );
  const list = h("ul", { class: "tracklist", role: "listbox", "aria-label": "Tracks", "aria-multiselectable": "true" });
  d.entries.forEach((e, i) => list.append(sheetRow(c, e, i)));
  body.append(list);
  if (c.inspected != null) {
    const e = entryOf(c, c.inspected);
    if (e) body.append(inspector(c, e, false));
  }
  return body;
}

function unavailableNote(entries) {
  const preview = entries.filter((e) => e.availability === "preview").length;
  const drm = entries.filter((e) => e.availability === "drm").length;
  const parts = [preview && `${preview} preview-only`, drm && `${drm} encrypted`].filter(Boolean).join(", ");
  return h("span", { class: "warn-note", title: drm ? "If these are your own tracks, enable downloads on them in the platform's track settings, then choose Original file." : "The platform only offers a short preview of these." }, `${parts}, can't be downloaded`);
}

function sheetRow(c, e, i) {
  const ok = selectable(e);
  const m = finalMeta(c, e);
  const box = h("input", { type: "checkbox", class: "tl-check", disabled: !ok, "aria-label": `Include ${m.title}` });
  box.checked = c.selected.has(e.index);
  box.addEventListener("click", (ev) => ev.stopPropagation());
  box.addEventListener("change", () => {
    box.checked ? c.selected.add(e.index) : c.selected.delete(e.index);
    renderSheet(true);
  });
  const loading = !e.detailed && c.data.enriching && (!e.title || e.title === "Untitled");
  const showArtist = !isRelease(c) || (m.artist && m.artist !== c.album.artist);
  const main = loading
    ? h("div", { class: "tl-main" }, h("span", { class: "skeleton w60" }), h("span", { class: "skeleton w30" }))
    : h("div", { class: "tl-main" }, h("span", { class: "tl-title" }, m.title), showArtist && h("span", { class: "tl-sub" }, m.artist));
  const flags = h("div", { class: "row" });
  if (c.overrides.has(e.index)) flags.append(h("span", { class: "flag edited" }, "Edited"));
  if (e.availability === "preview") flags.append(h("span", { class: "flag warn" }, "Preview only"));
  if (e.availability === "drm") flags.append(h("span", { class: "flag warn", title: "If it's your own track, turn on downloads for it in the platform's track settings (or enable “Use my browser sign-in”), then choose Original file to get your uploaded master." }, "Encrypted"));
  if (e.availability === "restricted") flags.append(h("span", { class: "flag", title: "May need your browser sign-in (Settings)" }, "Sign-in"));
  if (e.media?.original) flags.append(h("span", { class: "flag", title: "The uploader offers their original file" }, "Original"));
  if ((m.lyrics || "").trim()) flags.append(h("span", { class: "flag", title: e.lyrics && !c.overrides.get(e.index)?.lyrics ? "Lyrics found in the description" : "Lyrics attached" }, "Lyrics"));
  const row = h("li", { class: `tl-row ${c.selected.has(e.index) ? "" : "off"}`, role: "option", "aria-selected": String(c.inspected === e.index), "data-index": e.index, onclick: () => { c.inspected = c.inspected === e.index ? null : e.index; renderSheet(true); } },
    box,
    h("span", { class: "tl-num" }, numbered(c) ? i + 1 : ""),
    main,
    flags,
    h("span", { class: "tl-dur" }, e.durationMs ? fmtDur(e.durationMs) : ""),
    h("button", { class: "icon-btn", "aria-label": `Get info for ${m.title}`, onclick: (ev) => { ev.stopPropagation(); c.inspected = e.index; renderSheet(true); } }, icon("info-circle"))
  );
  return row;
}

const FIELDS = [
  ["title", "Title", "full"], ["artist", "Artist", "full"], ["album", "Album", "half"], ["albumArtist", "Album artist", "half"],
  ["trackNumber", "Track", "num"], ["trackTotal", "of", "num"], ["discNumber", "Disc", "num"], ["date", "Year or date", ""],
  ["genre", "Genre", "half"], ["composer", "Composer", "half"], ["label", "Label", "half"], ["isrc", "ISRC", ""], ["bpm", "BPM", "num"],
  ["key", "Key", ""], ["comment", "Comment", "full"],
];

function inspector(c, e, inline) {
  const m = finalMeta(c, e);
  const base = defaults(c, e);
  const fields = h("div", { class: "fields" });
  for (const [key, label, kind] of FIELDS) {
    const value = m[key] ?? "";
    const input = h("input", { value: String(value), inputmode: kind === "num" ? "numeric" : null, spellcheck: "false", "aria-label": label });
    input.classList.toggle("changed", String(base[key] ?? "") !== String(value));
    input.addEventListener("change", () => {
      let v = input.value.trim();
      if (kind === "num") v = v === "" ? null : Math.max(0, parseInt(v, 10) || 0);
      setOverride(c, e.index, key, v);
      input.classList.toggle("changed", String(base[key] ?? "") !== String(v ?? ""));
      if (key === "title" || key === "artist") renderSheet(true);
      else updateCommit();
    });
    fields.append(h("label", { class: `field ${kind === "full" ? "full" : kind === "half" ? "half" : ""}` }, h("span", {}, label), input));
  }
  const trim = c.trims.get(e.index) || { start: "", end: "" };
  const trimIn = (k, ph) => {
    const inp = h("input", { value: trim[k], placeholder: ph, spellcheck: "false", "aria-label": k === "start" ? "Start time" : "End time" });
    inp.addEventListener("change", () => {
      const t = { ...(c.trims.get(e.index) || { start: "", end: "" }), [k]: inp.value.trim() };
      t.start || t.end ? c.trims.set(e.index, t) : c.trims.delete(e.index);
    });
    return inp;
  };
  const artKind = (m.artwork || {}).kind || "keep";
  const coverSrc = artKind === "file" ? c.album.preview : e.thumbnail;
  const coverRow = h("div", { class: "ins-cover" },
    artKind === "none" ? h("div", { class: "ph" }, icon("photo")) : art(coverSrc, "", "Cover"),
    h("div", { class: "stack" },
      h("strong", {}, artKind === "file" ? "Your image" : artKind === "none" ? "No cover art" : "Cover from source"),
      h("div", { class: "row" },
        h("button", { class: "btn small", onclick: async () => { const p = await invoke("pick_image").catch((x) => toast(errText(x), "err")); if (p) { setOverride(c, e.index, "artwork", { kind: "file", path: p.path }); c.album.preview = c.album.preview || p.dataUrl; renderSheet(true); } } }, "Choose…"),
        artKind !== "none" ? h("button", { class: "btn small ghost", onclick: () => { setOverride(c, e.index, "artwork", { kind: "none" }); renderSheet(true); } }, "Remove") : h("button", { class: "btn small ghost", onclick: () => { setOverride(c, e.index, "artwork", { kind: "keep" }); renderSheet(true); } }, "Restore")
      )
    )
  );
  const avail = h("div", { class: "avail" }, ...availabilityFlags(e));
  const lyrBox = h("textarea", { class: "text area lyr-input", rows: "7", spellcheck: "false", placeholder: "Paste lyrics here. Add [mm:ss.xx] timings at the start of lines to make them synced." }, m.lyrics || "");
  const lyrStatus = h("span", { class: "muted small" });
  const describe = () => {
    const v = lyrBox.value.trim();
    const lines = v ? v.split(/\n/).filter((x) => x.trim()) : [];
    const timed = lines.filter((x) => /^\s*\[\d{1,3}:\d{1,2}([.:]\d{1,3})?\]/.test(x)).length;
    const fromDesc = e.lyrics && v === e.lyrics.trim() && !c.overrides.get(e.index)?.lyrics;
    lyrStatus.textContent = !v ? "No lyrics attached" : `${timed >= 2 ? `Synced · ${timed} timed lines` : `Plain · ${lines.length} lines`}${fromDesc ? " · found in the description" : ""}`;
  };
  describe();
  lyrBox.addEventListener("input", describe);
  lyrBox.addEventListener("change", () => { setOverride(c, e.index, "lyrics", lyrBox.value); renderSheet(true); });
  const lyrSection = h("div", { class: "ins-section" },
    h("div", { class: "ins-sub" }, h("h3", {}, "Lyrics"), lyrStatus),
    lyrBox,
    h("div", { class: "row" },
      h("button", { class: "btn small", onclick: async () => { try { const t = await invoke("pick_text_file"); if (t != null) { setOverride(c, e.index, "lyrics", t); renderSheet(true); } } catch (x) { toast(errText(x), "err"); } } }, icon("file-music"), "Import .lrc or .txt…"),
      lyrBox.value ? h("button", { class: "btn small ghost", onclick: () => { setOverride(c, e.index, "lyrics", ""); renderSheet(true); } }, "Clear") : null),
    h("p", { class: "muted small" }, "Saved with the download: plain lyrics go into the file's tags, and timed lyrics also become a .lrc file next to the song."));
  const descText = e.description || "";
  const descSection = descText ? h("details", { class: "ins-section desc" }, h("summary", {}, h("h3", {}, e.lyrics ? "Rest of the description" : "Description")), h("p", { class: "desc-text" }, descText)) : null;
  const ins = h("aside", { class: `inspector ${inline ? "inline" : ""}`, "aria-label": "Track details" });
  if (!inline) ins.append(h("div", { class: "ins-head" }, h("span", { class: "t" }, m.title), h("button", { class: "icon-btn", "aria-label": "Close", onclick: () => { c.inspected = null; renderSheet(true); } }, icon("x"))));
  ins.append(
    h("div", { class: "ins-body" },
      h("div", { class: "ins-section" }, h("h3", {}, "Tags"), fields),
      h("div", { class: "ins-section" }, h("h3", {}, "Cover art"), coverRow),
      lyrSection,
      descSection,
      h("div", { class: "ins-section" }, h("h3", {}, "Trim (optional)"), h("div", { class: "fields" }, h("label", { class: "field" }, h("span", {}, "Start"), trimIn("start", "0:00")), h("label", { class: "field" }, h("span", {}, "End"), trimIn("end", e.durationMs ? fmtDur(e.durationMs) : "end")))),
      h("div", { class: "ins-section" }, h("h3", {}, "What's available"), avail)
    ),
    h("div", { class: "ins-foot" },
      h("button", { class: "btn small ghost", disabled: !c.overrides.has(e.index), onclick: () => { c.overrides.delete(e.index); renderSheet(true); } }, "Reset edits"),
      !inline && h("button", { class: "btn small", onclick: () => { c.inspected = null; renderSheet(true); } }, "Done")
    )
  );
  return ins;
}

function availabilityFlags(e) {
  if (!e.media) return [h("span", { class: "flag" }, e.detailed ? "Unknown formats" : "Checking…")];
  const m = e.media, out = [];
  if (m.bestCodec) out.push(h("span", { class: "flag" }, `${codecName(m.bestCodec)}${m.bestAbr ? ` ${m.bestAbr} kbps` : ""} best audio`));
  if (m.original) out.push(h("span", { class: "flag" }, `Original ${m.originalExt ? m.originalExt.toUpperCase() : "file"}`));
  if (m.lossless) out.push(h("span", { class: "flag" }, "Lossless source"));
  if (m.hasVideo) out.push(h("span", { class: "flag" }, m.heights.length ? `Video up to ${m.heights[0]}p` : "Video"));
  if (e.availability !== "ok") out.push(h("span", { class: "flag warn" }, { drm: "Encrypted stream: if it's yours, enable downloads and use Original file", preview: "Preview only", restricted: "Needs sign-in" }[e.availability] || e.availability));
  return out;
}

/* save-as */
const MP3 = [64, 96, 128, 160, 192, 256, 320];
const snap = (req, cap) => MP3.filter((b) => b <= (cap ? Math.min(req, cap) : req)).pop() || 64;

function selectionCaps(c) {
  const ents = selectedEntries(c);
  const known = ents.filter((e) => e.media);
  const uncapped = (m) => m.lossless || (m.original && m.originalLossless) || !m.bestAbr;
  const abrs = known.filter((e) => !uncapped(e.media)).map((e) => e.media.bestAbr);
  const free = known.filter((e) => uncapped(e.media)).length;
  const lossless = known.some((e) => e.media.lossless || (e.media.original && e.media.originalLossless));
  const originals = known.filter((e) => e.media.original).length;
  const heights = [...new Set(known.flatMap((e) => e.media.heights || []))].sort((a, b) => b - a);
  const codecs = [...new Set(known.map((e) => e.media.bestCodec && `${codecName(e.media.bestCodec)}${e.media.bestAbr ? ` ${e.media.bestAbr} kbps` : ""}`).filter(Boolean))];
  return {
    count: ents.length,
    unknown: known.length < ents.length,
    maxAbr: free || !abrs.length ? null : Math.max(...abrs),
    minAbr: abrs.length ? Math.min(...abrs) : null,
    cappedCount: abrs.length,
    losslessCount: known.filter((e) => e.media.lossless || (e.media.original && e.media.originalLossless)).length,
    mixed: abrs.length > 0 && (free > 0 || Math.min(...abrs) < Math.max(...abrs)),
    lossless,
    originals,
    anyVideo: known.some((e) => e.media.hasVideo) || (known.length === 0 && ents.length > 0),
    heights,
    codec: codecs.length === 1 ? codecs[0] : null,
  };
}

function saveLabel(caps) {
  const s = state.settings;
  switch (s.saveAs) {
    case "original": return "Original file";
    case "mp3": return `MP3 · ${caps.mixed ? "up to " : ""}${snap(s.mp3Bitrate, caps.maxAbr)} kbps`;
    case "m4a": return "M4A (AAC)";
    case "flac": return "FLAC";
    case "wav": return "WAV";
    case "video": return `Video · ${s.videoHeight ? Math.min(s.videoHeight, caps.heights[0] || s.videoHeight) + "p" : "best"} ${s.videoContainer.toUpperCase()}`;
    default: return "Best quality";
  }
}

async function patchSave(patch) {
  try {
    state.settings = await invoke("save_settings", { patch });
  } catch (e) {
    toast(errText(e), "err");
  }
  updateCommit();
  renderSaveMenu();
}

function opt(value, title, detail, extra, disabled = false) {
  const on = state.settings.saveAs === value;
  return h("div", {},
    h("button", { class: "opt", role: "menuitemradio", "aria-checked": String(on), disabled, onclick: () => patchSave({ saveAs: value }) }, h("span", { class: "radio" }), h("span", {}, h("strong", {}, title), h("small", {}, detail))),
    on && extra ? h("div", { style: "padding: 0 12px 10px 42px" }, extra) : null
  );
}

function renderSaveMenu() {
  const menu = $("#saveMenu");
  const c = current();
  if (menu.hidden || !c) return;
  const caps = selectionCaps(c);
  const s = state.settings;
  const mp3Options = MP3.filter((b) => b >= 96 && (!caps.maxAbr || b <= snap(320, caps.maxAbr)));
  const eff = snap(s.mp3Bitrate, caps.maxAbr);
  const mp3Chips = h("div", {},
    h("div", { class: "chips" }, ...mp3Options.map((b) => h("button", { class: "chip", "aria-pressed": String(b === eff), onclick: () => patchSave({ mp3Bitrate: b }) }, `${b} kbps`))),
    (caps.maxAbr || caps.mixed) && h("small", { style: "display:block;color:var(--text-3);margin-top:6px" },
      caps.mixed
        ? `${plural(caps.cappedCount, "track")} only ${caps.cappedCount === 1 ? "has" : "have"} a ${caps.minAbr}${caps.maxAbr && caps.maxAbr !== caps.minAbr ? `–${caps.maxAbr}` : ""} kbps source, so ${caps.cappedCount === 1 ? "it's" : "they're"} encoded to match. Higher bitrates wouldn't add quality there.`
        : `Up to ${snap(320, caps.maxAbr)} kbps, the best ${caps.count > 1 ? "these sources offer" : "this source offers"}.`)
  );
  const heights = caps.heights.length ? caps.heights.filter((x) => x >= 360) : [2160, 1440, 1080, 720, 480];
  const vid = h("div", {},
    h("div", { class: "chips" }, h("button", { class: "chip", "aria-pressed": String(s.videoHeight === 0), onclick: () => patchSave({ videoHeight: 0 }) }, "Best"), ...heights.map((x) => h("button", { class: "chip", "aria-pressed": String(s.videoHeight === x), onclick: () => patchSave({ videoHeight: x }) }, `${x}p`))),
    h("div", { class: "chips" }, ...["mp4", "mkv"].map((k) => h("button", { class: "chip", "aria-pressed": String(s.videoContainer === k), onclick: () => patchSave({ videoContainer: k }) }, k.toUpperCase())))
  );
  const lossless = h("div", { class: "chips" }, ...["flac", "wav"].map((k) => h("button", { class: "chip", "aria-pressed": String(s.saveAs === k), onclick: () => patchSave({ saveAs: k }) }, k.toUpperCase())));
  const noFf = !state.tools?.ffmpeg.path;
  const ffNote = noFf ? " Needs ffmpeg." : "";
  const origDetail = caps.originals === 0 ? (caps.unknown ? "Checking whether the uploader offers one…" : "Not offered by the uploader") : caps.count === 1 ? "The uploader's own file, untouched" : `Offered for ${caps.originals} of ${caps.count}; the rest use best quality`;
  const lossDetail = caps.losslessCount === 0
    ? `Uncompressed files, but no better than the source.${ffNote}`
    : caps.losslessCount === caps.count
      ? "Bit-for-bit from the lossless source."
      : `Lossless for ${caps.losslessCount} of ${caps.count}; the rest can't exceed their ${caps.minAbr || "stream"} kbps source.`;
  menu.replaceChildren(...[
    opt("best", "Best quality", caps.codec ? `Kept exactly as delivered: ${caps.codec}` : "Each item kept exactly as delivered, no re-encoding"),
    opt("original", "Original file", origDetail, null, caps.originals === 0 && !caps.unknown),
    h("div", { class: "menu-divider" }),
    opt("mp3", "MP3", `Plays everywhere.${ffNote}`, mp3Chips, noFf),
    opt("m4a", "M4A (AAC)", `Apple devices and iTunes. AAC sources are copied, not re-encoded.${ffNote}`, null, noFf),
    opt(s.saveAs === "wav" ? "wav" : "flac", "Lossless: FLAC or WAV", lossDetail, lossless, noFf),
    caps.anyVideo ? h("div", { class: "menu-divider" }) : null,
    caps.anyVideo ? opt("video", "Video", caps.heights.length ? `Up to ${caps.heights[0]}p available${ffNote}` : `Picture and sound.${ffNote}`, vid, noFf) : null,
  ].filter(Boolean));
}

function openSaveMenu() {
  const menu = $("#saveMenu");
  menu.hidden = false;
  $("#saveAsBtn").setAttribute("aria-expanded", "true");
  renderSaveMenu();
  const r = $("#saveAsBtn").getBoundingClientRect();
  menu.style.left = `${r.left}px`;
  menu.style.top = `${Math.max(12, r.top - menu.offsetHeight - 8)}px`;
  menu.querySelector(".opt[aria-checked='true']")?.focus();
}

function closeSaveMenu() {
  $("#saveMenu").hidden = true;
  $("#saveAsBtn").setAttribute("aria-expanded", "false");
}

/* commit bar */
function bindCommit() {
  $("#saveAsBtn").addEventListener("click", () => ($("#saveMenu").hidden ? openSaveMenu() : closeSaveMenu()));
  $("#destBtn").addEventListener("click", chooseFolder);
  $("#downloadBtn").addEventListener("click", startDownload);
  $("#attest").addEventListener("change", () => $(".attest").classList.remove("nudge"));
}

async function chooseFolder() {
  const path = await invoke("pick_folder").catch(() => null);
  if (!path) return;
  state.settings = await invoke("save_settings", { patch: { outputDir: path } });
  updateCommit();
  renderSettings();
}

function updateCommit() {
  const c = current();
  if (!c || !state.settings) return;
  const caps = selectionCaps(c);
  const n = caps.count;
  const sep = state.settings.outputDir.includes("/") ? "/" : "\\";
  const folder = state.settings.collectionFolders ? folderFor(c) : null;
  $("#saveAsValue").textContent = saveLabel(caps);
  const sel = $("#destPlaylist");
  const lists = Lib.data?.playlists || [];
  const keep = sel.value;
  sel.replaceChildren(h("option", { value: "" }, "No playlist"), ...lists.map((p) => h("option", { value: p.name }, `+ ${p.name}`)));
  sel.value = lists.some((p) => p.name === keep) ? keep : "";
  sel.hidden = lists.length === 0;
  $("#destPath").textContent = folder ? `${state.settings.outputDir}${sep}${folder}` : state.settings.outputDir;
  $("#attestText").textContent = n > 1 ? `I have the rights to these ${n}` : "I have the rights to this";
  const btn = $("#downloadBtn");
  btn.disabled = n === 0 || !state.tools?.ytdlp.path;
  const noun = c.data.single ? (c.data.entries[0]?.media?.hasVideo && state.settings.saveAs === "video" ? "video" : "track") : isRelease(c) ? "track" : "item";
  btn.textContent = n === 0 ? "Select something" : n === 1 ? "Download" : `Download ${n} ${noun}s`;
  renderSaveMenu();
}

async function startDownload() {
  const c = current();
  if (!c || $("#downloadBtn").disabled) return;
  if (!$("#attest").checked) {
    const a = $(".attest");
    a.classList.remove("nudge");
    void a.offsetWidth;
    a.classList.add("nudge");
    $("#attest").focus();
    return;
  }
  const ents = selectedEntries(c);
  const folder = state.settings.collectionFolders ? folderFor(c) : null;
  const playlist = $("#destPlaylist").value || null;
  const jobs = ents.map((e) => ({ entry: e, meta: finalMeta(c, e), folder, trim: c.trims.get(e.index) || null, playlist }));
  const s = state.settings;
  try {
    const ids = await invoke("enqueue", { jobs, save: { saveAs: s.saveAs, mp3Bitrate: s.mp3Bitrate, videoHeight: s.videoHeight, videoContainer: s.videoContainer }, attested: true });
    const group = { key: `${c.data.id}-${Date.now()}`, title: c.data.single ? jobs[0].meta.title : c.album.title || c.data.title, artist: c.data.single ? jobs[0].meta.artist : c.album.artist, thumb: c.album.preview || c.data.thumbnail, single: ids.length === 1, ids, started: Date.now() };
    ids.forEach((id, i) => {
      const job = { id, req: jobs[i], save: { ...s }, group: group.key, num: numbered(c) ? position(c, ents[i]) : null, title: jobs[i].meta.title, state: "queued", ev: null };
      state.jobs.set(id, job);
      const early = state.early.get(id);
      if (early) {
        state.early.delete(id);
        applyJobEvent(job, early);
      }
    });
    state.groups.unshift(group);
    fly($("#sheetCover"));
    $("#attest").checked = false;
    updateNav();
    if (state.view === "downloads") renderDownloads();
  } catch (e) {
    toast(errText(e), "err");
  }
}

function fly(from) {
  const nav = $("#navDownloads");
  const land = () => {
    nav.classList.remove("landed");
    void nav.offsetWidth;
    nav.classList.add("landed");
  };
  if (!from || reduced.matches) return land();
  const a = from.getBoundingClientRect(), b = nav.querySelector(".nav-ring").getBoundingClientRect();
  const clone = from.tagName === "IMG" ? h("img", { src: from.src, alt: "" }) : h("div", { style: "background:var(--surface-2)" });
  clone.className = "flyer";
  Object.assign(clone.style, { left: `${a.left}px`, top: `${a.top}px`, width: `${a.width}px`, height: `${a.height}px` });
  document.body.append(clone);
  const dx = b.left + b.width / 2 - (a.left + a.width / 2), dy = b.top + b.height / 2 - (a.top + a.height / 2), sc = 20 / a.width;
  const anim = clone.animate([{ transform: "translate(0, 0) scale(1)", opacity: 1 }, { transform: `translate(${dx * 0.55}px, ${dy * 0.35 - 40}px) scale(${(1 + sc) / 2})`, opacity: 0.95, offset: 0.55 }, { transform: `translate(${dx}px, ${dy}px) scale(${sc})`, opacity: 0.2 }], { duration: 480, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" });
  anim.onfinish = () => {
    clone.remove();
    land();
  };
}

/* jobs */
function onJob(ev) {
  const job = state.jobs.get(ev.id);
  if (!job) return state.early.set(ev.id, ev);
  applyJobEvent(job, ev);
}

function applyJobEvent(job, ev) {
  const was = job.state;
  job.state = ev.state;
  job.ev = ev;
  if (ev.state === "done" && was !== "done") state.historyDirty = true;
  scheduleDownloads();
}

let dlQueued = false;
function scheduleDownloads() {
  if (dlQueued) return;
  dlQueued = true;
  requestAnimationFrame(() => {
    dlQueued = false;
    updateNav();
    if (state.view === "downloads") renderDownloads();
  });
}

function groupStats(g) {
  const jobs = g.ids.map((id) => state.jobs.get(id)).filter(Boolean);
  const done = jobs.filter((j) => j.state === "done" || j.state === "skipped").length;
  const failed = jobs.filter((j) => j.state === "error").length;
  const cancelled = jobs.filter((j) => j.state === "cancelled").length;
  const active = jobs.filter((j) => ACTIVE.has(j.state)).length;
  const progress = jobs.length ? jobs.reduce((a, j) => a + (FINAL.has(j.state) ? 1 : j.ev?.progress || 0), 0) / jobs.length : 0;
  return { jobs, done, failed, cancelled, active, progress };
}

function updateNav() {
  const groups = state.groups.map(groupStats);
  const active = groups.reduce((a, g) => a + g.active, 0);
  const live = groups.filter((g) => g.active > 0);
  const p = live.length ? live.reduce((a, g) => a + g.progress * g.jobs.length, 0) / live.reduce((a, g) => a + g.jobs.length, 0) : 0;
  $("#navDownloads").classList.toggle("busy", active > 0);
  $("#navRing").style.strokeDashoffset = String(53.4 * (1 - p));
  $("#navCount").hidden = active === 0;
  $("#navCount").textContent = String(active);
}

function jobStatus(j) {
  const ev = j.ev || {};
  const fmt = ev.format ? `${ev.format} · ` : "";
  switch (j.state) {
    case "queued": return ["Waiting", ""];
    case "preparing": return ["Preparing…", ""];
    case "downloading": {
      const size = ev.total ? `${fmtBytes(ev.received)} of ${fmtBytes(ev.total)}` : fmtBytes(ev.received);
      return [`${size}${ev.speed ? ` · ${fmtBytes(ev.speed)}/s` : ""}`, ""];
    }
    case "processing": return [`${fmt}Converting and embedding…`, ""];
    case "tagging": return ["Writing your tags…", ""];
    case "done": return [`Saved · ${ev.format || ""}`, "ok"];
    case "skipped": return [ev.message || "Already downloaded", "ok"];
    case "cancelled": return ["Cancelled", ""];
    case "error": return [ev.message || "Failed", "err"];
    default: return [j.state, ""];
  }
}

function jobRow(j) {
  const [text, cls] = jobStatus(j);
  const ev = j.ev || {};
  const indet = j.state === "preparing" || j.state === "processing" || j.state === "tagging" || (j.state === "downloading" && !ev.total && !ev.progress);
  const right = h("div", { class: "job-right" }, h("span", { class: `job-status ${cls}`, title: text }, text));
  if (ACTIVE.has(j.state) && j.state !== "queued") right.append(progressBar(indet ? null : ev.progress || 0));
  const acts = ACTIVE.has(j.state)
    ? h("button", { class: "icon-btn", "aria-label": `Cancel ${j.title}`, onclick: () => invoke("cancel", { id: j.id }) }, icon("x"))
    : j.state === "error" || j.state === "cancelled"
      ? h("button", { class: "icon-btn", "aria-label": `Retry ${j.title}`, onclick: () => retry([j]) }, icon("refresh"))
      : ev.path ? h("button", { class: "icon-btn", "aria-label": "Show in folder", onclick: () => invoke("reveal", { path: ev.path }) }, icon("folder")) : h("span");
  const li = h("li", { class: "job" }, h("span", { class: "tl-num" }, j.num ?? ""), h("span", { class: "job-title", title: j.title }, j.title), right, acts);
  if (j.state === "done" && ev.message) li.append(h("div", { class: "job-note" }, ev.message));
  return li;
}

async function retry(jobs) {
  if (!jobs.length) return;
  try {
    const s = jobs[0].save;
    const ids = await invoke("enqueue", { jobs: jobs.map((j) => j.req), save: { saveAs: s.saveAs, mp3Bitrate: s.mp3Bitrate, videoHeight: s.videoHeight, videoContainer: s.videoContainer }, attested: true });
    jobs.forEach((j, i) => {
      const g = state.groups.find((x) => x.key === j.group);
      state.jobs.delete(j.id);
      const nj = { ...j, id: ids[i], state: "queued", ev: null };
      state.jobs.set(ids[i], nj);
      if (g) g.ids = g.ids.map((x) => (x === j.id ? ids[i] : x));
    });
    scheduleDownloads();
  } catch (e) {
    toast(errText(e), "err");
  }
}

async function renderDownloads(refresh = false) {
  const groups = state.groups.map((g) => ({ g, st: groupStats(g) }));
  const attention = groups.filter(({ st }) => st.active > 0 || st.failed > 0 || st.cancelled > 0);
  const active = groups.reduce((a, x) => a + x.st.active, 0);
  $("#downloadsSub").textContent = active ? `${plural(active, "download")} in progress` : attention.length ? "Some downloads need attention" : "Nothing in progress";
  $("#cancelAllBtn").hidden = active === 0;
  $("#activeGroups").replaceChildren(...attention.map(({ g, st }) => groupCard(g, st)));
  if (refresh || state.historyDirty) {
    state.historyDirty = false;
    state.history = await invoke("history_list").catch(() => []);
    renderHistory();
  }
  $("#emptyDownloads").hidden = attention.length > 0 || state.history.length > 0;
  $("#historySection").hidden = state.history.length === 0;
}

function groupCard(g, st) {
  const n = st.jobs.length;
  const sub = st.active ? `${st.done} of ${n} done · ${Math.round(st.progress * 100)}%` : [st.done && `${st.done} saved`, st.failed && `${st.failed} failed`, st.cancelled && `${st.cancelled} cancelled`].filter(Boolean).join(" · ");
  const failed = st.jobs.filter((j) => j.state === "error" || j.state === "cancelled");
  const action = st.active
    ? h("button", { class: "btn small ghost", onclick: () => st.jobs.filter((j) => ACTIVE.has(j.state)).forEach((j) => invoke("cancel", { id: j.id })) }, "Cancel")
    : h("div", { class: "row" },
        failed.length > 0 && h("button", { class: "btn small", onclick: () => retry(failed) }, icon("refresh"), failed.length > 1 ? `Retry ${failed.length}` : "Retry"),
        h("button", { class: "btn small ghost", onclick: () => { state.groups = state.groups.filter((x) => x !== g); renderDownloads(); } }, "Dismiss"));
  const card = h("article", { class: "group-card" },
    h("div", { class: "gc-head" }, art(g.thumb, "thumb"), h("div", { style: "min-width:0" }, h("div", { class: "gc-title" }, g.title), h("div", { class: "gc-sub" }, [g.artist, sub].filter(Boolean).join(" · "))), action)
  );
  if (n > 1) {
    card.append(h("div", { class: "gc-bar" }, progressBar(st.progress, st.failed && !st.active ? "err" : "")));
    card.append(h("ul", { class: "jobs" }, ...st.jobs.map(jobRow)));
  } else if (st.jobs[0]) {
    card.append(h("ul", { class: "jobs" }, jobRow(st.jobs[0])));
  }
  return card;
}

function renderHistory() {
  const list = $("#historyList");
  const out = [];
  let day = null;
  for (const r of state.history.slice(0, 400)) {
    const d = dayLabel(r.at);
    if (d !== day) {
      day = d;
      out.push(h("h3", { class: "day" }, d));
    }
    const missing = !r.path;
    out.push(
      h("div", { class: `hist ${missing ? "missing" : ""}`, "data-hist": r.id },
        art(r.thumbnail, "thumb"),
        h("div", { style: "min-width:0" }, h("div", { class: "t" }, r.title), h("div", { class: "m" }, [r.artist, r.format, missing ? "File moved or deleted" : null].filter(Boolean).join(" · "))),
        h("span", { class: "when" }, timeFmt.format(new Date(r.at))),
        h("div", { class: "acts" },
          !missing && h("button", { class: "icon-btn", "aria-label": "Show in folder", onclick: () => invoke("reveal", { path: r.path }) }, icon("folder")),
          r.url && h("button", { class: "icon-btn", "aria-label": "Open source page", onclick: () => invoke("open_link", { url: r.url }) }, icon("external-link")),
          h("button", { class: "icon-btn", "aria-label": "Remove from history", onclick: async () => { await invoke("history_remove", { ids: [r.id] }); state.history = state.history.filter((x) => x !== r); renderHistory(); } }, icon("trash")))
      )
    );
  }
  list.replaceChildren(...out);
}

/* settings */
let savedTimer;
async function patch(p) {
  try {
    state.settings = await invoke("save_settings", { patch: p });
    applyTheme();
    const f = $("#savedFlag");
    f.textContent = "Saved";
    f.classList.add("on");
    clearTimeout(savedTimer);
    savedTimer = setTimeout(() => f.classList.remove("on"), 1400);
    updateCommit();
    if ("ytdlpPath" in p || "ffmpegPath" in p) refreshTools();
  } catch (e) {
    toast(errText(e), "err");
  }
}

const TOKENS = ["{artist}", "{title}", "{album}", "{albumartist}", "{index}", "{year}", "{genre}", "{collection}", "{platform}"];
function bindSettings() {
  $$("[data-key]").forEach((el) => {
    const key = el.dataset.key;
    el.addEventListener("change", () => patch({ [key]: el.type === "checkbox" ? el.checked : el.value.trim() }));
  });
  bindSignin();
  $("#templateInput").addEventListener("input", renderTemplatePreview);
  $("#tokens").replaceChildren(
    ...TOKENS.map((t) =>
      h("button", { type: "button", onclick: () => {
        const inp = $("#templateInput");
        const at = inp.selectionStart ?? inp.value.length;
        inp.value = inp.value.slice(0, at) + t + inp.value.slice(inp.selectionEnd ?? at);
        inp.focus();
        inp.setSelectionRange(at + t.length, at + t.length);
        renderTemplatePreview();
        patch({ filenameTemplate: inp.value });
      } }, t)
    )
  );
  $$(".stepper button").forEach((b) => b.addEventListener("click", () => patch({ concurrency: Math.max(1, Math.min(8, state.settings.concurrency + Number(b.dataset.step))) }).then(renderSettings)));
  $$("[data-seg=theme] button").forEach((b) => b.addEventListener("click", () => patch({ theme: b.dataset.value }).then(renderSettings)));
  $("#setDirBtn").addEventListener("click", chooseFolder);
  $("#openFolderBtn").addEventListener("click", () => invoke("reveal", { path: state.settings.outputDir }));
  $("#cancelAllBtn").addEventListener("click", () => invoke("cancel_all"));
  $("#clearHistory").addEventListener("click", async () => {
    await invoke("history_clear");
    state.history = [];
    renderDownloads();
  });
  $("#revokeBtn").addEventListener("click", async () => {
    state.settings = await invoke("revoke_rights");
    $$(".pledge-check").forEach((c) => (c.checked = false));
    $("#obAccept").disabled = true;
    showOnboarding(1);
  });
}

function renderTemplatePreview() {
  const t = $("#templateInput").value || "{artist} - {title}";
  const c = current();
  const e = c?.data.entries.find((x) => c.selected.has(x.index)) || c?.data.entries[0];
  const m = c && e ? finalMeta(c, e) : { artist: "Nova Lumen", title: "Night Drive", album: "Glass", albumArtist: "Nova Lumen", trackNumber: 3, trackTotal: 12, date: "2021", genre: "Ambient" };
  const idx = m.trackNumber ? String(m.trackNumber).padStart(Math.max(2, String(m.trackTotal || "").length), "0") : "";
  let name = t.replace("{artist}", m.artist).replace("{title}", m.title).replace("{album}", m.album || "").replace("{albumartist}", m.albumArtist || "").replace("{index}", idx).replace("{year}", (m.date || "").slice(0, 4)).replace("{genre}", m.genre || "").replace("{collection}", c ? folderFor(c) || "" : "Glass").replace("{platform}", e?.platform || "SoundCloud");
  if (state.settings.numberTracks && idx && !t.includes("{index}") && (!c || folderFor(c))) name = `${idx} - ${name}`;
  $("#templatePreview").textContent = `${name.replace(/[<>:"/\\|?*]/g, "_")}.mp3`;
}

function renderSettings() {
  const s = state.settings;
  if (!s) return;
  $$("[data-key]").forEach((el) => {
    const v = s[el.dataset.key];
    if (el.type === "checkbox") el.checked = !!v;
    else if (document.activeElement !== el) el.value = v ?? "";
  });
  $("#setDir").textContent = s.outputDir;
  $("#concValue").textContent = String(s.concurrency);
  $$("[data-seg=theme] button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.value === s.theme)));
  const at = s.rightsAcceptedAt ? new Intl.DateTimeFormat(undefined, { dateStyle: "long" }).format(new Date(s.rightsAcceptedAt * 1000)) : "";
  $("#rightsInfo").textContent = at ? `You agreed on ${at}. Withdrawing it cancels downloads and asks again in the app.` : "Not confirmed yet.";
  renderTemplatePreview();
  renderSignin();
  if (typeof renderPlaybackSettings === "function") renderPlaybackSettings();
  if (typeof renderApiSettings === "function") renderApiSettings();
  renderToolSettings();
  renderLibrarySettings();
  renderLyricsSettings();
  renderStorage();
}

const OS = isMac ? "mac" : /Linux|X11/.test(navigator.userAgent) ? "linux" : "windows";

function signinStatus(text, kind) {
  const el = $("#signinStatus");
  if (!el) return;
  el.hidden = !text;
  el.textContent = text || "";
  el.dataset.kind = kind || "";
}

function renderSignin() {
  const s = state.settings;
  const sel = $("#signinSelect");
  if (!s || !sel) return;
  $$("option[data-os]", sel).forEach((o) => (o.hidden = o.dataset.os !== OS && o.value !== s.cookiesBrowser));
  const isFile = s.cookiesBrowser === "file";
  $("#cookiesFileRow").hidden = !isFile;
  $("#signinCheck").hidden = !s.cookiesBrowser;
  $("#cookiesFilePath").textContent = s.cookiesFile || "Export one in Netscape format from a browser you're signed in to, using a cookies.txt extension.";
  $("#cookiesFilePath").title = s.cookiesFile || "";
}

async function checkSignin() {
  const b = $("#signinCheck");
  b.classList.add("loading");
  b.disabled = true;
  signinStatus("Checking…", "");
  try {
    signinStatus(await invoke("signin_check", { browser: state.settings.cookiesBrowser, file: state.settings.cookiesFile || "" }), "ok");
  } catch (e) {
    signinStatus(errText(e), "err");
  } finally {
    b.disabled = false;
    b.classList.remove("loading");
  }
}

function bindSignin() {
  $("#signinSelect")?.addEventListener("change", async (e) => {
    const v = e.target.value;
    signinStatus("");
    await patch({ cookiesBrowser: v });
    renderSignin();
    if (v === "file" && !state.settings.cookiesFile) pickCookies();
    else if (v) checkSignin();
  });
  $("#signinCheck")?.addEventListener("click", checkSignin);
  $("#cookiesFilePick")?.addEventListener("click", pickCookies);
}

async function pickCookies() {
  const path = await invoke("pick_cookies_file").catch((e) => (toast(errText(e), "err"), null));
  if (!path) return;
  await patch({ cookiesFile: path });
  renderSignin();
  checkSignin();
}

function renderToolSettings() {
  const t = state.tools;
  if (!t || !$("#toolYtdlp")) return;
  const card = (key, tool, name, desc) => {
    const busy = state.installing[key];
    const status = tool.path ? `${tool.version ? tool.version + " · " : ""}${tool.managed ? "Managed by Atmosphere" : "Your copy"}` : desc;
    const acts = h("div", { class: "acts" });
    if (busy) acts.append(progressBar(busy.total ? busy.received / busy.total : null));
    else if (!tool.path) acts.append(h("button", { class: "btn primary", disabled: key === "ffmpeg" && !t.canInstallFfmpeg, onclick: () => installTool(key) }, "Install"));
    else if (key === "ytdlp") acts.append(h("button", { class: "btn", onclick: async (ev) => { const b = ev.currentTarget; b.classList.add("loading"); b.disabled = true; try { toast(await invoke("update_ytdlp"), "ok"); await refreshTools(); } catch (e) { toast(errText(e), "err"); } finally { b.disabled = false; b.classList.remove("loading"); } } }, h("span", { class: "label" }, "Check for update"), h("span", { class: "spin" })));
    else if (tool.managed) acts.append(h("button", { class: "btn", onclick: () => installTool(key) }, "Reinstall"));
    return [h("div", {}, h("strong", { class: "name" }, h("span", { class: `dot ${tool.path ? "ok" : "warn"}` }), name), h("span", {}, status)), acts];
  };
  $("#toolYtdlp").replaceChildren(...card("ytdlp", t.ytdlp, "yt-dlp", "Not installed. Needed to read any link."));
  $("#toolFfmpeg").replaceChildren(...card("ffmpeg", t.ffmpeg, "ffmpeg", t.canInstallFfmpeg ? "Not installed. Needed for MP3, M4A, FLAC, WAV, video and cover art." : isMac ? "Install it with Homebrew (brew install ffmpeg). Atmosphere finds it automatically." : "Install it with your package manager, then restart."));
}

window.addEventListener("DOMContentLoaded", () => init().catch((e) => toast(errText(e), "err")));
