// atmosphere · connect.js · Settings and player bridge for the opt-in local API other apps can use
const Connect = { timer: null, beat: null };
const API_DOCS = "https://github.com/pvrzz/atmosphere/blob/main/docs/api.md";

function apiSnapshot() {
  const t = Player.current;
  const a = Player.audio;
  return {
    playing: !!t && !a.paused,
    track: t ? { id: t.id, title: t.title, artist: t.artist, album: t.album, albumArtist: t.albumArtist, durationMs: t.durationMs, artwork: t.art ? `/v1/artwork/${t.id}` : null } : null,
    positionMs: Math.round((a.currentTime || 0) * 1000),
    updatedAt: Date.now(),
    volume: Player.volume,
    muted: !!Player.muted,
    shuffle: !!Player.shuffle,
    repeat: Player.repeat || "off",
    queue: { index: Player.index, length: Player.queue.length },
  };
}

function apiPublishSoon() {
  if (!state.settings?.apiEnabled) return;
  clearTimeout(Connect.timer);
  Connect.timer = setTimeout(() => invoke("api_publish", { state: apiSnapshot() }).catch(() => {}), 120);
}

function ago(secs) {
  const d = Math.max(0, Date.now() / 1000 - secs);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(new Date(secs * 1000));
}

async function renderApiSettings() {
  const el = $("#apiStatus");
  if (!el || !state.settings) return;
  const st = await invoke("api_status").catch(() => null);
  if (!st) return;
  const s = state.settings;
  el.dataset.kind = "";
  if (!s.apiEnabled) el.textContent = "Off. No other app can reach Atmosphere.";
  else if (st.error) {
    el.textContent = st.error;
    el.dataset.kind = "err";
  } else if (st.running) {
    el.textContent = `On at http://127.0.0.1:${st.port}${st.lastUsed ? ` · last used ${ago(st.lastUsed)}${st.lastClient ? ` by ${st.lastClient}` : ""}` : " · not used yet"}`;
    el.dataset.kind = "ok";
  } else el.textContent = "Starting…";
  $("#apiTokenBtn").textContent = st.hasToken ? "Replace token" : "Create token";
  $("#apiTokenNote").textContent = st.hasToken
    ? `Created ${ago(st.tokenCreated)}. Replacing it disconnects every app using the old one.`
    : "Apps send this token with every request. It's shown once; Atmosphere keeps only a fingerprint.";
  const port = $("#apiPort");
  if (document.activeElement !== port) port.value = String(s.apiPort || 47823);
  $$(".api-scope").forEach((row) => row.classList.toggle("dim", !s.apiEnabled));
}

async function newApiToken() {
  const had = (await invoke("api_status").catch(() => null))?.hasToken;
  if (had && !(await confirmBox("Replace the access token?", "Apps using the current token stop working until you give them the new one.", "Replace"))) return;
  try {
    const token = await invoke("api_new_token");
    $("#apiTokenValue").textContent = token;
    $("#apiTokenBox").hidden = false;
    renderApiSettings();
  } catch (e) {
    toast(errText(e), "err");
  }
}

function runApiAction(p) {
  switch (p.action) {
    case "status": return renderApiSettings();
    case "play": return Player.play();
    case "pause": return Player.pause();
    case "toggle": return Player.toggle();
    case "next": return Player.next();
    case "previous": return Player.prev();
    case "seek": return Player.seek(p.position);
    case "volume": Player.muted = false; return Player.setVolume(p.volume);
    case "queue": {
      const tracks = (p.ids || []).map((id) => Lib.byId.get(id)).filter(Boolean);
      if (!tracks.length) return;
      if (p.mode === "now") return Player.setQueue(tracks, 0);
      if (p.mode === "next") return Player.playNext(tracks);
      return Player.addToQueue(tracks);
    }
    case "link":
      if (Mini?.on) closeMini();
      go("get");
      fetchText(p.url);
      return toast("Another app sent a link. Review it before saving anything.", "");
  }
}

function bindConnect() {
  listen("atmosphere://api", (e) => runApiAction(e.payload || {}));
  window.addEventListener("player", (e) => {
    if (e.detail.type !== "time") apiPublishSoon();
  });
  Player.audio.addEventListener("seeked", apiPublishSoon);
  Connect.beat = setInterval(() => {
    if (!Player.audio.paused) apiPublishSoon();
  }, 10000);
  $("#apiTokenBtn")?.addEventListener("click", newApiToken);
  $("#apiTokenCopy")?.addEventListener("click", async () => {
    await invoke("clipboard_write", { text: $("#apiTokenValue").textContent }).catch(() => {});
    toast("Token copied", "ok");
  });
  $("#apiDocs")?.addEventListener("click", () => invoke("open_link", { url: API_DOCS }).catch(() => {}));
  $("#apiEnabledToggle")?.addEventListener("change", () => setTimeout(() => { renderApiSettings(); apiPublishSoon(); }, 400));
  $("#apiPort")?.addEventListener("change", async (e) => {
    const n = parseInt(e.target.value, 10);
    if (!(n >= 1024 && n <= 65535)) {
      toast("Choose a port between 1024 and 65535.", "err");
      e.target.value = String(state.settings.apiPort || 47823);
      return;
    }
    await patch({ apiPort: n });
    setTimeout(renderApiSettings, 400);
  });
  renderApiSettings();
}
