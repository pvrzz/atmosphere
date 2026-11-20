// atmosphere · discord.js · Shares what's playing with Discord and runs the Discord settings
const Dsc = { timer: 0, last: "" };
const DISCORD_PORTAL = "https://discord.com/developers/applications";

function discordSnapshot() {
  const t = Player.current;
  if (!t) return null;
  const a = Player.audio;
  const dur = Number.isFinite(a.duration) && a.duration > 0 ? a.duration * 1000 : t.durationMs || 0;
  return { title: t.title || "", artist: t.artist || "", album: t.album || "", durationMs: Math.round(dur), positionMs: Math.round((a.currentTime || 0) * 1000), playing: !a.paused };
}

function discordSoon(force = false) {
  clearTimeout(Dsc.timer);
  Dsc.timer = setTimeout(() => {
    const now = discordSnapshot();
    const key = JSON.stringify(now && { ...now, positionMs: Math.round(now.positionMs / 2000) });
    if (!force && key === Dsc.last) return;
    Dsc.last = key;
    invoke("discord_now", { now }).catch(() => {});
  }, 300);
}

const DISCORD_TEXT = {
  off: ["Off. Nothing is shared with Discord.", ""],
  noid: ["Add your Discord application ID below to turn this on.", "err"],
  connecting: ["Connecting to Discord…", ""],
  waiting: ["Waiting for Discord. Open the Discord app on this computer and it connects by itself.", ""],
};

function renderDiscordStatus(st) {
  const el = $("#discordStatus");
  if (!el || !st) return;
  const [text, kind] = DISCORD_TEXT[st.state] || [st.error || "Couldn't reach Discord.", "err"];
  if (st.state === "connected") {
    el.textContent = st.error ? `Connected, but Discord said: ${st.error}` : `Connected${st.user ? ` as ${st.user}` : ""}. Your status shows what you're playing.`;
    el.dataset.kind = st.error ? "err" : "ok";
  } else {
    el.textContent = text;
    el.dataset.kind = kind;
  }
  $("#discordClientId").placeholder = st.builtinId ? "Atmosphere's own" : "Required";
}

async function renderDiscordSettings() {
  const s = state.settings;
  if (!s || !$("#discordStatus")) return;
  $$("[data-seg=discordStatus] button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.value === (s.discordStatus || "app"))));
  $$(".discord-opt").forEach((row) => row.classList.toggle("dim", !s.discordEnabled));
  const id = $("#discordClientId");
  if (document.activeElement !== id) id.value = s.discordClientId || "";
  renderDiscordStatus(await invoke("discord_status").catch(() => null));
}

async function saveDiscordId(e) {
  const v = e.target.value.trim();
  if (v && !/^\d{15,25}$/.test(v)) {
    toast("An application ID is a long number. Copy it from your app's General Information page.", "err");
    e.target.value = state.settings.discordClientId || "";
    return;
  }
  if (v === (state.settings.discordClientId || "")) return;
  await patch({ discordClientId: v });
  renderDiscordSettings();
}

async function saveDiscordAssets(btn) {
  btn.classList.add("loading");
  try {
    const dir = await invoke("discord_save_assets");
    if (dir) {
      toast("Saved the images and a short how-to", "ok");
      invoke("reveal", { path: dir }).catch(() => {});
    }
  } catch (e) {
    toast(errText(e), "err");
  } finally {
    btn.classList.remove("loading");
  }
}

function bindDiscord() {
  window.addEventListener("player", (e) => {
    const type = e.detail.type;
    if (type === "track" || type === "state") discordSoon();
  });
  Player.on("seeked", () => discordSoon(true));
  Player.on("loadedmetadata", () => discordSoon(true));
  setInterval(() => discordSoon(), 15000);
  listen("atmosphere://discord", (e) => renderDiscordStatus(e.payload));
  $("#discordToggle")?.addEventListener("change", () => setTimeout(() => { renderDiscordSettings(); discordSoon(true); }, 300));
  $$("[data-seg=discordStatus] button").forEach((b) => b.addEventListener("click", () => patch({ discordStatus: b.dataset.value }).then(renderDiscordSettings)));
  $("#discordClientId")?.addEventListener("change", saveDiscordId);
  $("#discordPortal")?.addEventListener("click", () => invoke("open_link", { url: DISCORD_PORTAL }).catch(() => {}));
  $("#discordAssets")?.addEventListener("click", (e) => saveDiscordAssets(e.currentTarget));
  discordSoon(true);
  renderDiscordSettings();
}
