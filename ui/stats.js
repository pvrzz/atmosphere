/* atmosphere · stats.js · Listening stats (week/month/year/all), top songs/artists/albums, and shareable social-size stat cards. */
const Stats = { period: "month", offset: 0, data: null };
const DAY = 86400000;

function periodRange(period, offset) {
  const now = new Date();
  if (period === "week") {
    const d = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    d.setDate(d.getDate() - ((d.getDay() + 6) % 7) + offset * 7);
    const end = new Date(d);
    end.setDate(end.getDate() + 7);
    const fmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
    return { start: d.getTime(), end: end.getTime(), label: offset === 0 ? "This week" : `${fmt.format(d)} – ${fmt.format(new Date(end.getTime() - DAY))}`, title: offset === 0 ? "My week" : `My week of ${fmt.format(d)}` };
  }
  if (period === "month") {
    const d = new Date(now.getFullYear(), now.getMonth() + offset, 1);
    const end = new Date(d.getFullYear(), d.getMonth() + 1, 1);
    const label = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" }).format(d);
    return { start: d.getTime(), end: end.getTime(), label, title: `My ${label}` };
  }
  if (period === "year") {
    const y = now.getFullYear() + offset;
    return { start: new Date(y, 0, 1).getTime(), end: new Date(y + 1, 0, 1).getTime(), label: String(y), title: `My ${y}` };
  }
  return { start: 0, end: Infinity, label: "All time", title: "My all time" };
}

function buckets(period, range) {
  if (period === "week") return [...Array(7)].map((_, i) => { const d = new Date(range.start + i * DAY); return { start: d.getTime(), end: d.getTime() + DAY, label: new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(d) }; });
  if (period === "month") {
    const d = new Date(range.start);
    const days = new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
    return [...Array(days)].map((_, i) => { const s = new Date(d.getFullYear(), d.getMonth(), i + 1); return { start: s.getTime(), end: s.getTime() + DAY, label: String(i + 1) }; });
  }
  if (period === "year") {
    const y = new Date(range.start).getFullYear();
    return [...Array(12)].map((_, i) => ({ start: new Date(y, i, 1).getTime(), end: new Date(y, i + 1, 1).getTime(), label: new Intl.DateTimeFormat(undefined, { month: "short" }).format(new Date(y, i, 1)) }));
  }
  const years = [...new Set((Stats.data?.listens || []).map((l) => new Date(l.at).getFullYear()))].sort();
  return (years.length ? years : [new Date().getFullYear()]).map((y) => ({ start: new Date(y, 0, 1).getTime(), end: new Date(y + 1, 0, 1).getTime(), label: String(y) }));
}

const isPlay = (l, t) => l.ms >= 30000 || (t?.durationMs && l.ms >= t.durationMs * 0.5);
const artistOf = (t) => t.albumArtist || primaryArtist(t.artist);
const dayKey = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

function fmtHours(ms) {
  const m = Math.round(ms / 60000);
  if (m < 60) return `${m} min`;
  const hh = Math.floor(m / 60);
  return m % 60 ? `${hh}h ${m % 60}m` : `${hh}h`;
}

function aggregate() {
  const range = periodRange(Stats.period, Stats.offset);
  const listens = (Stats.data?.listens || []).filter((l) => l.at >= range.start && l.at < range.end && Lib.byId.has(l.id));
  const songs = new Map(), artists = new Map(), albums = new Map();
  let ms = 0, plays = 0;
  for (const l of listens) {
    const t = Lib.byId.get(l.id);
    const p = isPlay(l, t) ? 1 : 0;
    ms += l.ms;
    plays += p;
    const bump = (map, key, extra) => { const e = map.get(key) || { key, plays: 0, ms: 0, ...extra }; e.plays += p; e.ms += l.ms; map.set(key, e); };
    bump(songs, t.id, { track: t });
    bump(artists, lnorm(artistOf(t)), { name: artistOf(t) });
    if (t.album) bump(albums, `${lnorm(artistOf(t))}|${lnorm(t.album)}`, { title: t.album, artist: artistOf(t), art: t.art });
  }
  const rank = (m) => [...m.values()].filter((e) => e.ms > 0).sort((a, b) => b.plays - a.plays || b.ms - a.ms);
  const bars = buckets(Stats.period, range).map((b) => ({ ...b, ms: listens.filter((l) => l.at >= b.start && l.at < b.end).reduce((n, l) => n + l.ms, 0) }));
  let appSecs = 0;
  for (const [day, secs] of Object.entries(Stats.data?.appTime || {})) {
    const t = new Date(`${day}T12:00:00`).getTime();
    if (t >= range.start && t < range.end) appSecs += secs;
  }
  return { range, ms, plays, songs: rank(songs), artists: rank(artists), albums: rank(albums), bars, appMs: appSecs * 1000 };
}

async function loadStats() {
  Stats.data = await invoke("stats_get").catch(() => ({ listens: [], appTime: {} }));
}

function statsView(root) {
  if (!Stats.data) {
    root.append(libHead("Stats", "Loading…"));
    loadStats().then(() => Lib.route.name === "stats" && renderLib(true));
    return;
  }
  const a = aggregate();
  const seg = h("div", { class: "seg", role: "tablist", "aria-label": "Period" }, ...[["week", "Week"], ["month", "Month"], ["year", "Year"], ["all", "All time"]].map(([id, label]) => h("button", { type: "button", role: "tab", "aria-pressed": String(Stats.period === id), onclick: () => { Stats.period = id; Stats.offset = 0; renderLib(true); } }, label)));
  const nav = Stats.period === "all" ? null : h("div", { class: "period-nav" },
    h("button", { class: "icon-btn", "aria-label": "Previous period", onclick: () => { Stats.offset--; renderLib(true); } }, icon("chevron-left")),
    h("span", { class: "period-label" }, a.range.label),
    h("button", { class: "icon-btn", "aria-label": "Next period", disabled: Stats.offset >= 0, onclick: () => { Stats.offset++; renderLib(true); } }, icon("chevron-right")));
  root.append(libHead("Stats", "Only songs still in your library are counted.", [seg, h("button", { class: "btn", disabled: !a.ms, onclick: () => shareStats(a) }, icon("share"), "Share")]));
  const body = h("div", { class: "pad-x stats" });
  if (nav) body.append(nav);
  if (!a.ms) {
    body.append(h("div", { class: "lib-empty slim" }, icon("chart-bar"), h("h2", {}, "Nothing played yet"), h("p", {}, Stats.offset ? "No listening in this period." : "Play some music and your stats will build up here.")));
    return root.append(body);
  }
  const card = (label, value, sub) => h("div", { class: "stat-card" }, h("span", { class: "stat-label" }, label), h("strong", { class: "stat-value" }, value), sub ? h("span", { class: "stat-sub" }, sub) : null);
  body.append(h("div", { class: "stat-cards" },
    card("Listening time", fmtHours(a.ms), `${Math.round(a.ms / 60000).toLocaleString()} minutes`),
    card("Plays", a.plays.toLocaleString(), a.plays ? `${fmtDur(a.ms / Math.max(1, a.plays))} average` : null),
    card("Songs", a.songs.length.toLocaleString(), `${a.artists.length.toLocaleString()} artists`),
    card("Time in Atmosphere", a.appMs ? fmtHours(a.appMs) : "—", "while the app was open")));
  const max = Math.max(...a.bars.map((b) => b.ms), 1);
  const chart = h("div", { class: "chart", role: "img", "aria-label": `Listening time per ${Stats.period === "year" ? "month" : Stats.period === "all" ? "year" : "day"}` },
    ...a.bars.map((b) => h("div", { class: "chart-col", title: `${b.label}: ${fmtHours(b.ms)}` }, h("div", { class: "chart-bar", style: `transform: scaleY(${(b.ms / max).toFixed(3)})` }), h("span", { class: "chart-x" }, a.bars.length > 16 && Number(b.label) % 5 !== 1 && b.label !== "1" ? "" : b.label))));
  body.append(h("section", { class: "stat-panel" }, h("h2", { class: "sec" }, "When you listened"), chart));
  const songRows = a.songs.slice(0, 10).map((e, i) => h("div", { class: "rank-row", role: "button", tabindex: "0", ondblclick: () => playList(a.songs.map((x) => x.track), i) },
    h("span", { class: "rank" }, String(i + 1)), cover(e.track.art, "lt-cover"),
    h("div", { class: "lt-main" }, h("span", { class: "lt-title" }, e.track.title), h("span", { class: "lt-sub" }, e.track.artist)),
    h("span", { class: "rank-meta" }, `${plural(e.plays, "play")} · ${fmtHours(e.ms)}`)));
  const artistRows = a.artists.slice(0, 8).map((e, i) => {
    const ar = Lib.artists.get(e.key);
    return h("div", { class: "rank-row", role: "button", tabindex: "0", onclick: () => ar && openLib("artist", ar.key) },
      h("span", { class: "rank" }, String(i + 1)), cover(ar ? artistImage(ar) : null, "lt-cover round", "", "user"),
      h("div", { class: "lt-main" }, h("span", { class: "lt-title" }, e.name), h("span", { class: "lt-sub" }, `${plural(e.plays, "play")} · ${fmtHours(e.ms)}`)));
  });
  body.append(h("div", { class: "stat-split" },
    h("section", { class: "stat-panel" }, h("h2", { class: "sec" }, "Top songs"), ...songRows),
    h("section", { class: "stat-panel" }, h("h2", { class: "sec" }, "Top artists"), ...artistRows)));
  if (a.albums.length) {
    body.append(h("section", { class: "stat-panel" }, h("h2", { class: "sec" }, "Top albums"),
      h("div", { class: "grid tight" }, ...a.albums.slice(0, 6).map((e) => { const al = Lib.albums.get(e.key); return al ? albumCard(al) : null; }).filter(Boolean))));
  }
  root.append(body);
}

/* share cards */
const SHARE_SIZES = [
  { id: "story", label: "Story", hint: "Instagram, TikTok, Snapchat", w: 1080, h: 1920 },
  { id: "square", label: "Square", hint: "Instagram post, Discord", w: 1080, h: 1080 },
  { id: "portrait", label: "Portrait", hint: "Instagram, Threads", w: 1080, h: 1350 },
  { id: "wide", label: "Wide", hint: "X, Bluesky, Reddit", w: 1600, h: 900 },
];
const Share = { size: "story", theme: "cover" };

function loadImg(src) {
  return new Promise((resolve) => {
    if (!src) return resolve(null);
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => resolve(img);
    img.onerror = () => resolve(null);
    img.src = src;
  });
}

function roundRect(g, x, y, w, hgt, r) {
  g.beginPath();
  g.roundRect(x, y, w, hgt, r);
}

function fitText(g, text, maxW) {
  if (g.measureText(text).width <= maxW) return text;
  let t = text;
  while (t.length > 1 && g.measureText(`${t}…`).width > maxW) t = t.slice(0, -1);
  return `${t}…`;
}

async function drawCard(canvas, a) {
  const size = SHARE_SIZES.find((s) => s.id === Share.size);
  canvas.width = size.w;
  canvas.height = size.h;
  const g = canvas.getContext("2d");
  const W = size.w, H = size.h, u = Math.min(W, H) / 1080, wide = W > H;
  const font = getComputedStyle(document.documentElement).getPropertyValue("--font-display").trim() || "system-ui";
  const topArt = a.albums[0]?.art || a.songs[0]?.track.art;
  const [bgImg, lockup, ...covers] = await Promise.all([Share.theme === "cover" ? loadImg(artUrl(topArt)) : null, loadImg("assets/lockup-light.svg"), ...a.songs.slice(0, 5).map((e) => loadImg(artUrl(e.track.art)))]);
  const sky = g.createRadialGradient(W * 0.5, H * 1.05, 0, W * 0.5, H * 1.05, Math.max(W, H) * 1.1);
  sky.addColorStop(0, "#3a1f9a");
  sky.addColorStop(0.38, "#1b1248");
  sky.addColorStop(0.75, "#0c0b22");
  sky.addColorStop(1, "#06070f");
  g.fillStyle = sky;
  g.fillRect(0, 0, W, H);
  if (bgImg) {
    g.save();
    g.filter = "blur(70px) saturate(1.4) brightness(0.55)";
    const s = Math.max(W / bgImg.width, H / bgImg.height) * 1.25;
    g.drawImage(bgImg, (W - bgImg.width * s) / 2, (H - bgImg.height * s) / 2, bgImg.width * s, bgImg.height * s);
    g.restore();
    const shade = g.createLinearGradient(0, 0, 0, H);
    shade.addColorStop(0, "rgba(6,7,15,0.25)");
    shade.addColorStop(1, "rgba(6,7,15,0.7)");
    g.fillStyle = shade;
    g.fillRect(0, 0, W, H);
  }
  const pad = 80 * u;
  if (lockup) {
    const lh = 64 * u;
    g.drawImage(lockup, pad, pad, (lockup.width / lockup.height) * lh, lh);
  }
  g.fillStyle = "#fff";
  g.textBaseline = "alphabetic";
  const colW = wide ? W * 0.42 : W - pad * 2;
  let y = pad + 64 * u + (wide ? 120 : 150) * u;
  g.font = `700 ${(wide ? 76 : 92) * u}px ${font}`;
  g.fillText(fitText(g, a.range.title, colW), pad, y);
  g.font = `500 ${34 * u}px ${font}`;
  g.fillStyle = "rgba(255,255,255,0.65)";
  y += 56 * u;
  g.fillText("on Atmosphere", pad, y);
  const stat = (label, value, x, yy) => {
    g.fillStyle = "rgba(255,255,255,0.6)";
    g.font = `600 ${26 * u}px ${font}`;
    g.fillText(label.toUpperCase(), x, yy);
    g.fillStyle = "#fff";
    g.font = `700 ${68 * u}px ${font}`;
    g.fillText(value, x, yy + 74 * u);
  };
  y += (wide ? 90 : 120) * u;
  stat("Listened", fmtHours(a.ms), pad, y);
  stat("Plays", a.plays.toLocaleString(), pad + colW * 0.55, y);
  y += 160 * u;
  if (a.artists[0]) {
    g.fillStyle = "rgba(255,255,255,0.6)";
    g.font = `600 ${26 * u}px ${font}`;
    g.fillText("TOP ARTIST", pad, y);
    g.fillStyle = "#fff";
    g.font = `700 ${56 * u}px ${font}`;
    g.fillText(fitText(g, a.artists[0].name, colW), pad, y + 66 * u);
    y += 140 * u;
  }
  const listX = wide ? W * 0.52 : pad;
  const listW = wide ? W - listX - pad : W - pad * 2;
  const n = wide ? 5 : Share.size === "square" ? 3 : 5;
  const rowH = wide ? 132 * u : Share.size === "square" ? 104 * u : Share.size === "story" ? 168 * u : 124 * u;
  const block = 30 * u + n * rowH;
  let ly = wide ? Math.max(pad + 40 * u, (H - block) / 2) : y + Math.max(20 * u, (H - pad - 60 * u - y - block) / 2);
  const coverS = rowH - 24 * u;
  g.fillStyle = "rgba(255,255,255,0.6)";
  g.font = `600 ${26 * u}px ${font}`;
  g.fillText("TOP SONGS", listX, ly);
  ly += 30 * u;
  a.songs.slice(0, n).forEach((e, i) => {
    const top = ly + i * rowH;
    g.fillStyle = "rgba(255,255,255,0.5)";
    g.font = `700 ${34 * u}px ${font}`;
    g.fillText(String(i + 1), listX, top + rowH / 2 + 12 * u);
    const cx = listX + 56 * u;
    g.save();
    roundRect(g, cx, top + 12 * u, coverS, coverS, 14 * u);
    g.clip();
    if (covers[i]) g.drawImage(covers[i], cx, top + 12 * u, coverS, coverS);
    else {
      g.fillStyle = "rgba(255,255,255,0.12)";
      g.fill();
    }
    g.restore();
    const tx = cx + coverS + 28 * u;
    const tw = listW - (tx - listX);
    g.fillStyle = "#fff";
    g.font = `650 ${38 * u}px ${font}`;
    g.fillText(fitText(g, e.track.title, tw), tx, top + rowH / 2 - 2 * u);
    g.fillStyle = "rgba(255,255,255,0.62)";
    g.font = `500 ${28 * u}px ${font}`;
    g.fillText(fitText(g, `${e.track.artist} · ${plural(e.plays, "play")}`, tw), tx, top + rowH / 2 + 38 * u);
  });
  g.fillStyle = "rgba(255,255,255,0.45)";
  g.font = `500 ${24 * u}px ${font}`;
  g.fillText(a.range.label, pad, H - pad + 10 * u);
}

async function shareStats(a) {
  const canvas = h("canvas", { class: "share-canvas", "aria-label": "Share card preview" });
  const redraw = async () => {
    await drawCard(canvas, a);
    sizeRow.querySelectorAll(".chip").forEach((c) => c.setAttribute("aria-pressed", String(c.dataset.id === Share.size)));
    themeRow.querySelectorAll(".chip").forEach((c) => c.setAttribute("aria-pressed", String(c.dataset.id === Share.theme)));
  };
  const sizeRow = h("div", { class: "chips" }, ...SHARE_SIZES.map((s) => h("button", { class: "chip", "data-id": s.id, title: `${s.w}×${s.h} · ${s.hint}`, onclick: () => { Share.size = s.id; redraw(); } }, s.label)));
  const themeRow = h("div", { class: "chips" }, h("button", { class: "chip", "data-id": "cover", onclick: () => { Share.theme = "cover"; redraw(); } }, "Cover art"), h("button", { class: "chip", "data-id": "night", onclick: () => { Share.theme = "night"; redraw(); } }, "Night sky"));
  const actions = h("div", { class: "row" },
    h("button", { class: "btn primary", onclick: async () => { try { const p = await invoke("image_save", { data: canvas.toDataURL("image/png"), name: `atmosphere-${Stats.period}-${Share.size}.png` }); if (p) toast("Saved your stats image", "ok"); } catch (e) { toast(errText(e), "err"); } } }, icon("photo-down"), "Save image…"),
    h("button", { class: "btn", onclick: async () => { try { await invoke("image_copy", { data: canvas.toDataURL("image/png") }); toast("Copied. Paste it anywhere", "ok"); } catch (e) { toast(errText(e), "err"); } } }, icon("copy"), "Copy"));
  const body = h("div", { class: "share" }, h("div", { class: "share-preview" }, canvas), h("div", { class: "share-side" }, h("h3", {}, "Size"), sizeRow, h("p", { class: "muted small" }, "Story 1080×1920 · Square 1080×1080 · Portrait 1080×1350 · Wide 1600×900"), h("h3", {}, "Look"), themeRow, actions));
  redraw();
  await modal("Share your stats", body, [{ label: "Done", value: true }]);
}

function bindStats() {
  window.addEventListener("played", () => { Stats.data = null; });
  listen("atmosphere://library", (e) => { if (e.payload.type === "changed") Stats.data = null; });
  setInterval(() => {
    if (document.visibilityState === "visible") invoke("app_heartbeat", { day: dayKey(new Date()), secs: 60 }).catch(() => {});
  }, 60000);
}
