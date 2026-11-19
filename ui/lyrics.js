/* atmosphere · lyrics.js · Synced lyrics panel, full-screen TV mode with visualizer, and the lyrics-provider settings. */
const Lyr = { cache: new Map(), tried: [], cachedMiss: false, data: null, trackId: null, loading: false, error: null, offset: 0, raf: 0, active: -1, tv: false, idleTimer: 0 };

const PROVIDER_INFO = {
  local: ["Your files", "Lyrics embedded in the song, or a .lrc file next to it."],
  lrclib: ["LRCLIB", "Free, community-synced lyrics library. No key needed."],
  musixmatch: ["Musixmatch", "Official developer API with your own key (free tier available). Synced lyrics depend on your plan."],
  netease: ["NetEase Cloud Music", "Free synced lyrics, often word-by-word with translations. No key needed."],
  kugou: ["Kugou", "Free synced lyrics for Chinese and Asian releases. Often blocked outside China, so it's off by default."],
};

function lyricsQuery(t) {
  return { title: t.title, artist: primaryArtist(t.artist), album: t.album || "", durationMs: t.durationMs || null, path: t.path };
}

async function loadLyrics(refresh = false) {
  const t = Player.current;
  if (!t) {
    Lyr.data = null;
    Lyr.trackId = null;
    return renderLyricsAll();
  }
  if (!refresh && Lyr.trackId === t.id && (Lyr.data || Lyr.loading)) return;
  Lyr.trackId = t.id;
  Lyr.error = null;
  Lyr.active = -1;
  if (!refresh && Lyr.cache.has(t.id)) {
    ({ lyrics: Lyr.data, tried: Lyr.tried, cached: Lyr.cachedMiss } = Lyr.cache.get(t.id));
    return renderLyricsAll();
  }
  Lyr.loading = true;
  Lyr.data = null;
  renderLyricsAll();
  try {
    const found = await invoke("lyrics_get", { query: lyricsQuery(t), refresh });
    Lyr.cache.set(t.id, found);
    if (Lyr.trackId === t.id) ({ lyrics: Lyr.data, tried: Lyr.tried, cached: Lyr.cachedMiss } = found);
  } catch (e) {
    if (Lyr.trackId === t.id) Lyr.error = errText(e);
  } finally {
    if (Lyr.trackId === t.id) Lyr.loading = false;
    renderLyricsAll();
  }
}

const blurOn = () => {
  try {
    return localStorage.getItem("atmo.lyricsBlur") !== "off";
  } catch {
    return true;
  }
};

function setDepth(box, idx) {
  $$(".lyr-line", box).forEach((el, i) => el.style.setProperty("--dist", String(Math.min(Math.abs(i - Math.max(idx, -1)) || 0, 4))));
}

function lyricLines(l, big) {
  const box = h("div", { class: `lyr ${big ? "big" : ""} ${l.synced ? "synced" : "plain"} ${l.synced && blurOn() ? "depth" : ""}` });
  l.lines.forEach((line, i) => {
    const el = h("div", { class: "lyr-line", "data-i": i, role: l.synced ? "button" : null, tabindex: l.synced ? "0" : null, onclick: () => line.t != null && Player.seek((line.t - Lyr.offset) / 1000) });
    if (line.words?.length > 1) line.words.forEach((w, j) => el.append(h("span", { class: "w", "data-j": j }, w.text)));
    else el.append(line.text || "· · ·");
    if (line.translation) el.append(h("span", { class: "lyr-tr" }, line.translation));
    if (l.synced) el.style.setProperty("--dist", String(Math.min(i + 1, 4)));
    box.append(el);
  });
  return box;
}

function lyricsBody(big) {
  const t = Player.current;
  if (!t) return h("div", { class: "lyr-state" }, h("p", {}, "Play something to see its lyrics."));
  if (Lyr.loading) return h("div", { class: "lyr-state" }, h("span", { class: "skeleton w60" }), h("span", { class: "skeleton w30" }), h("span", { class: "skeleton w60" }));
  if (Lyr.error) return h("div", { class: "lyr-state" }, h("p", {}, Lyr.error), h("button", { class: "btn small", onclick: () => loadLyrics(true) }, "Try again"));
  const l = Lyr.data;
  if (!l) return h("div", { class: "lyr-state" }, h("p", {}, "No lyrics found for this song."),
    Lyr.cachedMiss ? h("p", { class: "muted small" }, "Searched recently with no match. Search again to retry every source.") : Lyr.tried.length ? h("ul", { class: "tried" }, ...Lyr.tried.map((a) => h("li", {}, h("strong", {}, a.provider), ` ${a.outcome}`))) : null,
    h("div", { class: "row" }, h("button", { class: "btn small", onclick: () => loadLyrics(true) }, icon("refresh"), "Search again"), h("button", { class: "btn small ghost", onclick: () => { closeTv(); go("settings"); $("#set-lyrics")?.scrollIntoView(); } }, "Lyrics sources")));
  if (l.instrumental) return h("div", { class: "lyr-state" }, icon("music"), h("p", {}, "Instrumental"));
  return lyricLines(l, big);
}

function renderLyricsPanel() {
  const panel = $("#lyricsPanel");
  if (panel.hidden) return;
  if (Player.current && Lyr.trackId !== Player.current.id) loadLyrics();
  const l = Lyr.data;
  const t = Player.current;
  const canSave = l?.synced && t && !["Local .lrc", "Embedded"].includes(l.provider);
  panel.replaceChildren(
    h("div", { class: "drawer-head" }, h("div", {}, h("h2", {}, "Lyrics"), l ? h("p", { class: "muted small" }, `${l.provider}${l.wordSynced ? " · word-synced" : l.synced ? " · synced" : ""}`) : null),
      h("div", { class: "row" },
        l?.synced ? h("div", { class: "offset", title: "Nudge timing if lyrics run early or late" }, h("button", { class: "icon-btn", "aria-label": "Lyrics earlier", onclick: () => nudge(-250) }, "−"), h("output", {}, `${Lyr.offset > 0 ? "+" : ""}${(Lyr.offset / 1000).toFixed(2)}s`), h("button", { class: "icon-btn", "aria-label": "Lyrics later", onclick: () => nudge(250) }, "+")) : null,
        canSave ? h("button", { class: "icon-btn", title: "Save as .lrc next to the song", "aria-label": "Save lyrics file", onclick: saveLrc }, icon("download")) : null,
        t ? h("button", { class: "icon-btn", title: "Edit or attach lyrics", "aria-label": "Edit lyrics", onclick: editLyrics }, icon("pencil")) : null,
        h("button", { class: "icon-btn", title: "Search again", "aria-label": "Search again", onclick: () => loadLyrics(true) }, icon("refresh")),
        h("button", { class: "icon-btn", "aria-label": "Close", onclick: closeDrawers }, icon("x")))),
    h("div", { class: "drawer-body lyr-scroll" }, lyricsBody(false), l?.copyright ? h("p", { class: "lyr-copy" }, l.copyright) : null)
  );
  Lyr.active = -1;
  startTick();
}

async function editLyrics() {
  const t = Player.current;
  if (!t) return;
  const l = Lyr.data;
  const start = l?.lines?.length ? (l.synced ? l.lines.map((x) => (x.t != null ? `[${String(Math.floor(x.t / 60000)).padStart(2, "0")}:${String(Math.floor(x.t / 1000) % 60).padStart(2, "0")}.${String(Math.floor((x.t % 1000) / 10)).padStart(2, "0")}]` : "") + x.text).join("\n") : l.lines.map((x) => x.text).join("\n")) : "";
  const box = h("textarea", { class: "text area lyr-input", rows: "14", spellcheck: "false", placeholder: "Paste lyrics. Start lines with [mm:ss.xx] to make them synced." }, start);
  const stamp = h("button", { class: "btn small", title: "Insert the current playback time at the cursor", onclick: () => {
    const ms = Math.max(0, Player.audio.currentTime * 1000);
    const tag = `[${String(Math.floor(ms / 60000)).padStart(2, "0")}:${String(Math.floor(ms / 1000) % 60).padStart(2, "0")}.${String(Math.floor((ms % 1000) / 10)).padStart(2, "0")}]`;
    const at = box.selectionStart;
    const lineStart = box.value.lastIndexOf("\n", at - 1) + 1;
    box.value = box.value.slice(0, lineStart) + tag + box.value.slice(lineStart).replace(/^\[\d{1,3}:\d{1,2}([.:]\d{1,3})?\]/, "");
    const next = box.value.indexOf("\n", lineStart);
    box.focus();
    box.setSelectionRange(next < 0 ? box.value.length : next + 1, next < 0 ? box.value.length : next + 1);
  } }, icon("clock"), "Stamp current time");
  const importBtn = h("button", { class: "btn small ghost", onclick: async () => { const text = await invoke("pick_text_file").catch((x) => (toast(errText(x), "err"), null)); if (text != null) box.value = text; } }, icon("file-music"), "Import .lrc or .txt…");
  const ok = await modal(`Lyrics · ${t.title}`, h("div", { class: "stack" }, box, h("div", { class: "row" }, stamp, importBtn), h("p", { class: "muted small" }, "Tip: play the song, put the cursor on a line, and press “Stamp current time” as it's sung. That's how you sync lyrics yourself.")), [{ label: "Cancel", value: false }, { label: "Save to song", value: true, primary: true }]);
  if (!ok) return;
  try {
    await invoke("lyrics_save_text", { id: t.id, text: box.value });
    Lyr.cache.delete(t.id);
    toast("Lyrics saved to the song", "ok");
    loadLyrics(true);
  } catch (e) {
    toast(errText(e), "err");
  }
}

function nudge(ms) {
  Lyr.offset += ms;
  renderLyricsPanel();
}

async function saveLrc() {
  try {
    await invoke("lyrics_save", { id: Player.current.id, lyrics: Lyr.data });
    toast("Saved lyrics next to the song", "ok");
  } catch (e) {
    toast(errText(e), "err");
  }
}

function openLyrics() {
  if ($("#lyricsPanel").hidden) $("#lyricsBtn").click();
}

function renderLyricsAll() {
  renderLyricsPanel();
  if (Lyr.tv) renderTv();
  if (typeof Mini !== "undefined" && Mini.on) renderMiniPanel();
}

function centerLine(el) {
  let box = el.parentElement;
  while (box && box !== document.body) {
    const style = getComputedStyle(box);
    if (/(auto|scroll)/.test(style.overflowY) && box.scrollHeight > box.clientHeight) break;
    box = box.parentElement;
  }
  if (!box || box === document.body) return;
  const target = el.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop - (box.clientHeight - el.offsetHeight) / 2;
  box.scrollTo({ top: Math.max(0, target), behavior: reduced.matches ? "auto" : "smooth" });
}

function tick() {
  Lyr.raf = 0;
  const visible = !$("#lyricsPanel").hidden || Lyr.tv || (typeof Mini !== "undefined" && Mini.on && Mini.panelShown && Mini.prefs.panel === "lyrics");
  if (!visible) return;
  const l = Lyr.data;
  if (l?.synced) {
    const now = Player.audio.currentTime * 1000 + Lyr.offset;
    let idx = -1;
    for (let i = 0; i < l.lines.length; i++) {
      if (l.lines[i].t != null && l.lines[i].t <= now) idx = i;
      else if (l.lines[i].t != null && l.lines[i].t > now) break;
    }
    const boxes = $$(".lyr.synced");
    if (idx !== Lyr.active) {
      Lyr.active = idx;
      for (const box of boxes) {
        $$(".lyr-line", box).forEach((el, i) => {
          el.classList.toggle("on", i === idx);
          el.classList.toggle("past", i < idx);
        });
        if (box.classList.contains("depth")) setDepth(box, idx);
        const cur = box.querySelector(".lyr-line.on");
        if (cur) centerLine(cur);
      }
    }
    const line = l.lines[idx];
    if (line?.words?.length > 1) {
      for (const box of boxes) {
        const el = box.querySelector(".lyr-line.on");
        el && $$(".w", el).forEach((w, j) => w.classList.toggle("sung", line.words[j].t <= now));
      }
    }
  }
  if (Lyr.tv) drawViz();
  Lyr.raf = requestAnimationFrame(tick);
}

function startTick() {
  if (!Lyr.raf) Lyr.raf = requestAnimationFrame(tick);
}

/* TV mode */
function renderTv() {
  const tv = $("#tv");
  const t = Player.current;
  const l = Lyr.data;
  const hasLyrics = !!(l && !l.instrumental && l.lines?.length);
  const showCol = !!t && (Lyr.loading || hasLyrics);
  tv.classList.toggle("no-lyrics", !showCol);
  const note = !t ? null : Lyr.loading ? null : l?.instrumental ? "Instrumental" : !hasLyrics ? (Lyr.error ? "Lyrics sources unreachable" : "No lyrics found for this song") : `Lyrics · ${l.provider}`;
  const bg = t?.art ? `background-image:url("${artUrl(t.art)}")` : "";
  const withVideo = !!(t?.video && Player.showVideo);
  tv.classList.toggle("video", withVideo);
  const layer = withVideo ? h("video", { class: "tv-video", id: "tvVideo", src: mediaUrl("v", t.id), playsinline: true, preload: "auto" }) : h("div", { class: "tv-bg", style: bg });
  if (withVideo) layer.muted = true;
  tv.replaceChildren(
    layer,
    h("button", { class: "icon-btn tv-close", "aria-label": "Exit TV mode", onclick: closeTv }, icon("minimize")),
    t?.video ? h("button", { class: `btn small tv-video-btn ${withVideo ? "on" : ""}`, onclick: () => { Player.showVideo = !Player.showVideo; renderTv(); updatePlayerBar(); } }, icon(withVideo ? "photo" : "video"), withVideo ? "Show cover" : "Show video") : null,
    h("div", { class: "tv-stage" },
      h("div", { class: "tv-now" },
        t ? cover(t.art, "tv-cover", t.album || t.title) : h("div", { class: "tv-cover ph" }, icon("music")),
        h("div", { class: "tv-title" }, t?.title || "Nothing playing"),
        h("div", { class: "tv-artist" }, t?.artist || ""),
        note ? h("div", { class: "tv-note" }, note) : null,
        h("div", { class: "tv-bar" }, h("i", { id: "tvProg" })),
        h("div", { class: "tv-controls" },
          h("button", { class: "icon-btn", "aria-label": "Previous", onclick: () => Player.prev() }, icon("player-skip-back-filled")),
          h("button", { class: "icon-btn tv-play", "aria-label": "Play or pause", onclick: () => Player.toggle() }, icon(Player.audio.paused ? "player-play-filled" : "player-pause-filled")),
          h("button", { class: "icon-btn", "aria-label": "Next", onclick: () => Player.next() }, icon("player-skip-forward-filled")))),
      showCol ? h("div", { class: "tv-lyrics" }, Lyr.loading ? h("div", { class: "tv-finding" }, h("span", { class: "skeleton w60" }), h("span", { class: "skeleton w30" }), h("p", {}, "Finding lyrics…")) : lyricsBody(true)) : null),
    h("canvas", { class: "tv-viz", id: "tvViz", width: "1600", height: "220", "aria-hidden": "true" })
  );
  Lyr.active = -1;
  startTick();
  if (withVideo) {
    layer.addEventListener("loadedmetadata", syncVideo, { once: true });
    syncVideo();
  }
}

function syncVideo() {
  const v = $("#tvVideo");
  if (!v) return;
  const a = Player.audio;
  v.playbackRate = a.playbackRate || 1;
  if (v.readyState >= 1 && Math.abs(v.currentTime - a.currentTime) > 0.25) v.currentTime = Math.min(a.currentTime, Math.max(0, (v.duration || a.currentTime) - 0.05));
  if (a.paused && !v.paused) v.pause();
  else if (!a.paused && v.paused) v.play().catch(() => {});
}

function toggleVideo() {
  if (!Player.current?.video) return;
  if (Lyr.tv && Player.showVideo) {
    Player.showVideo = false;
    renderTv();
  } else {
    Player.showVideo = true;
    if (Lyr.tv) renderTv();
    else openTv();
  }
  updatePlayerBar();
}

function drawViz() {
  const bar = $("#tvProg");
  const a = Player.audio;
  if (bar && isFinite(a.duration)) bar.style.transform = `scaleX(${a.currentTime / a.duration})`;
  const c = $("#tvViz");
  const an = Player.nodes?.analyser;
  if (!c || !an) return;
  const g = c.getContext("2d");
  const data = new Uint8Array(an.frequencyBinCount);
  an.getByteFrequencyData(data);
  g.clearRect(0, 0, c.width, c.height);
  const n = 64;
  const w = c.width / n;
  const accent = getComputedStyle(document.documentElement).getPropertyValue("--accent").trim() || "#9aa4ff";
  g.fillStyle = accent;
  g.globalAlpha = 0.55;
  for (let i = 0; i < n; i++) {
    const v = data[Math.floor((i / n) * data.length * 0.75)] / 255;
    const hgt = Math.max(4, v * c.height * 0.9);
    const x = i * w + w * 0.3;
    g.beginPath();
    g.roundRect(x, c.height - hgt, w * 0.4, hgt, w * 0.2);
    g.fill();
  }
}

function wakeTv() {
  const tv = $("#tv");
  tv.classList.remove("idle");
  clearTimeout(Lyr.idleTimer);
  Lyr.idleTimer = setTimeout(() => tv.classList.add("idle"), 2800);
}

const winApi = () => window.__TAURI__?.window;

async function monitors() {
  try {
    return (await winApi()?.availableMonitors?.()) || [];
  } catch {
    return [];
  }
}

async function moveToMonitor(m) {
  const api = winApi();
  const dpi = window.__TAURI__?.dpi;
  if (!api || !dpi || !m) return;
  const win = api.getCurrentWindow();
  Lyr.restore = { pos: await win.outerPosition(), size: await win.outerSize(), max: await win.isMaximized() };
  if (Lyr.restore.max) await win.unmaximize();
  await win.setPosition(new dpi.PhysicalPosition(m.position.x + 40, m.position.y + 40));
}

async function restoreWindow() {
  const r = Lyr.restore;
  Lyr.restore = null;
  const api = winApi();
  const dpi = window.__TAURI__?.dpi;
  if (!r || !api || !dpi) return;
  const win = api.getCurrentWindow();
  await win.setPosition(new dpi.PhysicalPosition(r.pos.x, r.pos.y));
  await win.setSize(new dpi.PhysicalSize(r.size.width, r.size.height));
  if (r.max) await win.maximize();
}

async function chooseTvScreen(anchor) {
  if (Lyr.tv) return closeTv();
  const list = await monitors();
  if (list.length < 2) return openTv();
  const here = await winApi().currentMonitor().catch(() => null);
  const label = (m, i) => `${m.name && !/^\\\\.\\/.test(m.name) ? m.name : `Display ${i + 1}`} · ${m.size.width}×${m.size.height}`;
  const r = anchor.getBoundingClientRect();
  const m = menuAt(r.left, r.top - 10, [
    { label: "This screen", run: () => openTv() },
    "-",
    ...list.map((mon, i) => ({ label: `${label(mon, i)}${here && mon.position.x === here.position.x && mon.position.y === here.position.y ? " (current)" : ""}`, run: () => openTv(mon) })),
  ]);
  m.style.top = `${Math.max(8, r.top - m.offsetHeight - 10)}px`;
}

async function openTv(monitor = null) {
  if (Lyr.tv) return closeTv();
  Lyr.tv = true;
  closeFx();
  closeMenus();
  if (monitor) await moveToMonitor(monitor).catch(() => {});
  $("#tv").hidden = false;
  if (Player.current) loadLyrics();
  renderTv();
  wakeTv();
  invoke("set_fullscreen", { on: true }).catch(() => {});
}

async function closeTv() {
  if (!Lyr.tv) return;
  Lyr.tv = false;
  $("#tv").hidden = true;
  await invoke("set_fullscreen", { on: false }).catch(() => {});
  await restoreWindow().catch(() => {});
}

/* lyrics settings */
function providerName(id) {
  if (id.startsWith("custom:")) return (state.settings.customLyrics || []).find((c) => `custom:${c.id}` === id)?.name || "Custom provider";
  return PROVIDER_INFO[id]?.[0] || id;
}

async function saveLyricsSettings(patchObj) {
  state.settings = await invoke("save_settings", { patch: patchObj });
  Lyr.cache.clear();
  renderLyricsSettings();
}

function renderLyricsSettings() {
  const s = state.settings;
  const box = $("#lyricsProviders");
  if (!s || !box) return;
  const list = s.lyricsProviders.filter((p) => !p.id.startsWith("custom:") || (s.customLyrics || []).some((c) => `custom:${c.id}` === p.id));
  const move = (i, d) => {
    const next = [...list];
    const [x] = next.splice(i, 1);
    next.splice(i + d, 0, x);
    saveLyricsSettings({ lyricsProviders: next });
  };
  box.replaceChildren(...list.map((p, i) => {
    const sw = h("input", { type: "checkbox", class: "switch", "aria-label": `Use ${providerName(p.id)}` });
    sw.checked = p.enabled;
    sw.addEventListener("change", () => saveLyricsSettings({ lyricsProviders: list.map((x) => (x.id === p.id ? { ...x, enabled: sw.checked } : x)) }));
    const needsKey = p.id === "musixmatch" && !s.musixmatchKey;
    return h("div", { class: "setting" },
      h("div", {}, h("strong", {}, `${i + 1}. ${providerName(p.id)}`), h("span", {}, needsKey ? "Add your Musixmatch API key below to use it." : PROVIDER_INFO[p.id]?.[1] || "Your custom source.")),
      h("div", { class: "row" },
        h("button", { class: "icon-btn", "aria-label": "Move up", disabled: i === 0, onclick: () => move(i, -1) }, "↑"),
        h("button", { class: "icon-btn", "aria-label": "Move down", disabled: i === list.length - 1, onclick: () => move(i, 1) }, "↓"),
        sw));
  }));
  const customs = s.customLyrics || [];
  $("#customProviders").replaceChildren(
    ...customs.map((c) => {
      const field = (key, label, attrs = {}) => {
        const el = attrs.multiline ? h("textarea", { class: "text area", rows: "2", spellcheck: "false", placeholder: attrs.placeholder || "" }, c[key] || "") : h("input", { class: "text", value: c[key] || "", type: attrs.type || "text", spellcheck: "false", placeholder: attrs.placeholder || "" });
        el.addEventListener("change", () => saveLyricsSettings({ customLyrics: customs.map((x) => (x.id === c.id ? { ...x, [key]: el.value } : x)) }));
        return h("label", { class: "field" }, h("span", {}, label), el);
      };
      const fmt = h("select", { "aria-label": "Response format" }, h("option", { value: "lrc", selected: c.format !== "json" }, "LRC or plain text"), h("option", { value: "json", selected: c.format === "json" }, "JSON"));
      fmt.addEventListener("change", () => saveLyricsSettings({ customLyrics: customs.map((x) => (x.id === c.id ? { ...x, format: fmt.value } : x)) }));
      return h("div", { class: "setting stack custom" },
        h("div", { class: "fields two" }, field("name", "Name", { placeholder: "e.g. QQ Music via my proxy" }), field("key", "Key or token", { type: "password" }), h("div", { class: "field full" }, field("url", "URL template", { placeholder: "https://example.com/lyrics?track={title}&artist={artist}&key={key}" })), h("div", { class: "field full" }, field("headers", "Headers (one per line)", { multiline: true, placeholder: "Authorization: Bearer {key}" })), h("label", { class: "field" }, h("span", {}, "Response"), fmt), c.format === "json" ? field("jsonPath", "Path to lyrics", { placeholder: "data.lyrics.lrc" }) : h("span")),
        h("div", { class: "row end" }, h("button", { class: "btn ghost danger", onclick: () => saveLyricsSettings({ customLyrics: customs.filter((x) => x.id !== c.id), lyricsProviders: s.lyricsProviders.filter((p) => p.id !== `custom:${c.id}`) }) }, "Remove")));
    }),
    h("div", { class: "setting" }, h("div", {}, h("strong", {}, "Add a custom source"), h("span", {}, "For QQ Music, Spicy Lyrics, BiniLyrics, a self-hosted server, or any service you have access to. Use {title}, {artist}, {album}, {duration} and {key} in the URL or headers.")),
      h("button", { class: "btn", onclick: () => { const id = String(Date.now()); saveLyricsSettings({ customLyrics: [...customs, { id, name: "", url: "", headers: "", format: "lrc", jsonPath: "", key: "" }], lyricsProviders: [...s.lyricsProviders, { id: `custom:${id}`, enabled: true }] }); } }, icon("plus"), "Add source"))
  );
}

function renderBlurSetting() {
  const sw = $("#lyricsBlurToggle");
  if (!sw) return;
  sw.checked = blurOn();
}

function bindLyrics() {
  $("#lyricsBlurToggle")?.addEventListener("change", (e) => {
    try {
      localStorage.setItem("atmo.lyricsBlur", e.target.checked ? "on" : "off");
    } catch {}
    $$(".lyr.synced").forEach((b) => b.classList.toggle("depth", e.target.checked));
  });
  renderBlurSetting();
  window.addEventListener("player", (e) => {
    if (e.detail.type === "track") {
      Lyr.offset = 0;
      if (!$("#lyricsPanel").hidden || Lyr.tv) loadLyrics();
    }
    if (e.detail.type === "state" && Lyr.tv) $(".tv-play")?.replaceChildren(icon(Player.audio.paused ? "player-play-filled" : "player-pause-filled"));
  });
  $("#tv").addEventListener("mousemove", wakeTv);
  document.addEventListener("keydown", (e) => { if (Lyr.tv && e.key === "Escape") closeTv(); });
}
