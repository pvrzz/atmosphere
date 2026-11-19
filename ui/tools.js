// atmosphere · tools.js · Everyday player tools: song info, save the queue as a playlist, sleep timer and resuming long tracks
const Sleep = { until: 0, endOfSong: false, timer: 0, tick: 0, fading: false };
const RESUME_KEY = "atmo.resume";
const RESUME_MIN_MS = 20 * 60 * 1000;

function fmtDate(ms) {
  return ms ? new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date(ms)) : "Never";
}

function songInfo(t) {
  const last = Lib.data?.lastPlayed?.[t.id];
  const rate = t.sampleRate ? `${(t.sampleRate / 1000).toFixed(t.sampleRate % 1000 ? 1 : 0)} kHz` : null;
  const ch = t.channels === 1 ? "mono" : t.channels === 2 ? "stereo" : t.channels ? `${t.channels} channels` : null;
  const rows = [
    ["Format", [String(t.format || "").toUpperCase(), t.bitrate ? `${t.bitrate} kbps` : null, rate, ch].filter(Boolean).join(" · ")],
    ["Length", fmtDur(t.durationMs || 0)],
    ["Size", t.size ? fmtBytes(t.size) : null],
    ["Album", [t.album, t.trackNo ? `track ${t.trackNo}${t.trackTotal ? ` of ${t.trackTotal}` : ""}` : null, t.discNo > 1 ? `disc ${t.discNo}` : null].filter(Boolean).join(" · ") || null],
    ["Year", t.year ? String(t.year) : null],
    ["Genre", t.genre || null],
    ["Plays", String(Lib.plays[t.id] || 0)],
    ["Last played", last ? fmtDate(last * (last < 1e12 ? 1000 : 1)) : "Never"],
    ["Added", t.added ? fmtDate(t.added * (t.added < 1e12 ? 1000 : 1)) : null],
    ["Lyrics", t.hasLyrics ? "Saved with the song" : "Not saved yet"],
    ["Loudness", typeof t.gainDb === "number" ? `${t.gainDb > 0 ? "+" : ""}${t.gainDb} dB to even out` : "Not measured yet"],
    ["Playlist", t.playlist || null],
  ].filter(([, v]) => v);
  const body = h("div", { class: "stack" },
    h("div", { class: "info-head" }, cover(t.art, "info-art", t.title), h("div", { class: "stack tight" }, h("strong", { class: "info-title" }, t.title), h("span", { class: "muted" }, t.artist || "Unknown artist"))),
    h("dl", { class: "info-grid" }, ...rows.flatMap(([k, v]) => [h("dt", {}, k), h("dd", {}, v)])),
    h("div", { class: "info-path" }, h("code", {}, t.path), h("button", { class: "btn small", type: "button", onclick: () => invoke("reveal", { path: t.path }) }, icon("folder-open"), "Show in folder")));
  modal("Song info", body, [{ label: "Edit info…", value: "edit" }, { label: "Close", value: false, primary: true }]).then((v) => v === "edit" && editInfo(t));
}

async function saveQueueAsPlaylist() {
  const tracks = Player.queue.filter(Boolean);
  if (!tracks.length) return toast("The queue is empty.", "");
  const d = new Date();
  const input = h("input", { class: "text wide", value: `Queue ${d.toLocaleDateString()}`, "aria-label": "Playlist name", spellcheck: "false" });
  const ok = await modal("Save queue as a playlist", h("div", { class: "stack" }, input, h("p", { class: "muted small" }, `${plural(tracks.length, "song")}, in the order they're queued.`)), [{ label: "Cancel", value: false }, { label: "Save", value: true, primary: true }]);
  const name = input.value.trim();
  if (!ok || !name) return;
  try {
    const created = await invoke("playlist_create", { name });
    await invoke("playlist_add", { name: created, ids: tracks.map((t) => t.id) });
    toast(`Saved “${created}”`, "ok");
  } catch (e) {
    toast(errText(e), "err");
  }
}

function sleepActive() {
  return Sleep.endOfSong || Sleep.until > Date.now();
}

function sleepLabel() {
  if (Sleep.endOfSong) return "Stops after this song";
  const left = Math.max(0, Sleep.until - Date.now());
  const m = Math.ceil(left / 60000);
  return m >= 60 ? `Stops in ${Math.floor(m / 60)} h ${m % 60} min` : `Stops in ${m} min`;
}

function renderSleep() {
  const b = $("#sleepBtn");
  if (!b) return;
  const on = sleepActive();
  b.setAttribute("aria-pressed", String(on));
  b.title = on ? `Sleep timer: ${sleepLabel().toLowerCase()}` : "Sleep timer";
}

function cancelSleep(quiet = false) {
  clearTimeout(Sleep.timer);
  clearInterval(Sleep.tick);
  Sleep.until = 0;
  Sleep.endOfSong = false;
  Player.stopAfterCurrent = false;
  renderSleep();
  if (!quiet) toast("Sleep timer off", "");
}

async function sleepNow() {
  if (Sleep.fading) return;
  Sleep.fading = true;
  const start = Player.volume;
  const steps = 40;
  for (let i = 1; i <= steps; i++) {
    await new Promise((r) => setTimeout(r, 200));
    Player.setVolume(start * (1 - i / steps));
  }
  Player.pause();
  Player.setVolume(start);
  Sleep.fading = false;
  cancelSleep(true);
  toast("Sleep timer stopped the music. Good night.", "");
}

function setSleep(minutes) {
  cancelSleep(true);
  if (minutes === "song") {
    Sleep.endOfSong = true;
    Player.stopAfterCurrent = true;
    Player.cancelTransition();
  } else {
    Sleep.until = Date.now() + minutes * 60000;
    Sleep.timer = setTimeout(sleepNow, Math.max(0, minutes * 60000 - 8000));
    Sleep.tick = setInterval(renderSleep, 30000);
  }
  renderSleep();
  toast(sleepLabel(), "ok");
}

function sleepMenu(e) {
  e.stopPropagation();
  const r = e.currentTarget.getBoundingClientRect();
  const opt = (label, v) => ({ label, run: () => setSleep(v) });
  menuAt(r.left, r.top - 8, [
    sleepActive() ? { label: `${sleepLabel()} · Turn off`, run: () => cancelSleep() } : null,
    sleepActive() ? "-" : null,
    opt("15 minutes", 15), opt("30 minutes", 30), opt("45 minutes", 45), opt("1 hour", 60), opt("2 hours", 120),
    opt("End of this song", "song"),
  ].filter(Boolean));
}

function resumeMap() {
  try {
    return JSON.parse(localStorage.getItem(RESUME_KEY) || "{}");
  } catch {
    return {};
  }
}

function saveResume() {
  const t = Player.current;
  const a = Player.audio;
  if (!t || (t.durationMs || 0) < RESUME_MIN_MS || !isFinite(a.duration)) return;
  const map = resumeMap();
  if (a.currentTime > 30 && a.currentTime < a.duration * 0.97) map[t.id] = Math.floor(a.currentTime);
  else delete map[t.id];
  const keys = Object.keys(map);
  if (keys.length > 60) delete map[keys[0]];
  try {
    localStorage.setItem(RESUME_KEY, JSON.stringify(map));
  } catch {}
}

function bindTools() {
  $("#sleepBtn")?.addEventListener("click", sleepMenu);
  let lastSave = 0;
  window.addEventListener("player", (e) => {
    const type = e.detail.type;
    if (type === "time" && Date.now() - lastSave > 10000) {
      lastSave = Date.now();
      saveResume();
    }
    if (type === "sleep" && Sleep.endOfSong) {
      cancelSleep(true);
      toast("Sleep timer stopped the music. Good night.", "");
    }
    if (type === "track") {
      if (Sleep.endOfSong && !Player.stopAfterCurrent) Player.stopAfterCurrent = true;
      const t = Player.current;
      const at = t && (t.durationMs || 0) >= RESUME_MIN_MS ? resumeMap()[t.id] : 0;
      if (at) {
        const seekTo = Math.max(0, at - 5);
        const apply = () => {
          if (Player.current?.id === t.id && Player.audio.currentTime < 5) {
            Player.seek(seekTo);
            toast(`Picked up where you left off (${fmtDur(seekTo * 1000)})`, "");
          }
        };
        Player.audio.readyState >= 1 ? apply() : Player.audio.addEventListener("loadedmetadata", apply, { once: true });
      }
    }
  });
  Player.on("pause", saveResume);
  renderSleep();
}
