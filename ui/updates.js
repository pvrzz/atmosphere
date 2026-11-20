// atmosphere · updates.js · Update chip, what's-new dialog and install progress for signed GitHub releases
const Upd = { info: null, app: null, busy: false, checkedAt: 0 };
const UPDATE_EVERY = 6 * 3600 * 1000;

async function checkUpdates(manual = false) {
  const btn = $("#updateBtn");
  if (manual && btn) {
    btn.classList.add("loading");
    btn.disabled = true;
  }
  try {
    Upd.checkedAt = Date.now();
    Upd.info = await invoke("update_check");
    renderUpdate();
    if (!manual) updateNotice();
    if (manual) {
      if (Upd.info) openUpdate();
      else toast("You're on the latest version.", "ok");
    }
  } catch (e) {
    if (manual) toast(errText(e), "err");
  } finally {
    if (manual && btn) {
      btn.disabled = false;
      btn.classList.remove("loading");
    }
  }
}

function renderUpdate() {
  const chip = $("#updateChip");
  const info = $("#updateInfo");
  if (chip) {
    chip.hidden = !Upd.info;
    if (Upd.info) chip.replaceChildren(icon("sparkles"), h("span", {}, `Update to ${Upd.info.version}`));
  }
  if (info && Upd.app) {
    info.textContent = Upd.info
      ? `Atmosphere ${Upd.app.version}. Version ${Upd.info.version} is available.`
      : `Atmosphere ${Upd.app.version}${Upd.app.canSelfUpdate ? "" : ". This copy updates by downloading the new version."}`;
  }
}

function noticedVersion() {
  try { return localStorage.getItem("atmo.updateNoticed"); } catch { return null; }
}

function updateNotice() {
  const u = Upd.info;
  if (!u || Upd.busy || $("#updateNotice") || noticedVersion() === u.version || (typeof Mini !== "undefined" && Mini.on)) return;
  try { localStorage.setItem("atmo.updateNoticed", u.version); } catch {}
  const first = (u.notes || "").split("\n").map((l) => l.replace(/^[-*#\s]+/, "").trim()).find(Boolean);
  const close = () => {
    el.classList.add("out");
    setTimeout(() => el.remove(), 200);
  };
  const el = h("div", { class: "update-notice", id: "updateNotice", role: "status" },
    h("span", { class: "un-icon" }, icon("sparkles")),
    h("div", { class: "un-text" }, h("strong", {}, `Atmosphere ${u.version} is out`), h("span", {}, first || "A new version is ready to install.")),
    h("div", { class: "un-actions" },
      h("button", { class: "btn small primary", type: "button", onclick: () => { close(); if (u.canInstall) installUpdate(); else invoke("open_link", { url: u.page }).catch(() => {}); } }, u.canInstall ? "Update and restart" : "Download"),
      h("button", { class: "btn small", type: "button", onclick: () => { close(); openUpdate(); } }, "What's new")),
    h("button", { class: "icon-btn un-close", type: "button", "aria-label": "Not now", title: "Not now. It stays in the sidebar.", onclick: close }, icon("x")));
  document.body.append(el);
}

async function openUpdate() {
  const u = Upd.info;
  if (!u || Upd.busy) return;
  const notes = (u.notes || "").trim();
  const body = h("div", { class: "stack" },
    h("p", { class: "muted" }, `You have ${u.current}.${u.date ? ` Released ${new Intl.DateTimeFormat(undefined, { dateStyle: "long" }).format(new Date(u.date.replace(" ", "T").replace(/ .*$/, "")))}.` : ""}`),
    notes ? h("div", { class: "update-notes" }, notes) : null,
    u.canInstall ? h("p", { class: "muted small" }, "Atmosphere restarts to finish. Downloads in progress stop, and your library, settings and stats stay as they are.") : h("p", { class: "muted small" }, "This copy can't replace itself, so the new version opens on GitHub for you to download."));
  const go = await modal(`Atmosphere ${u.version} is here`, body, [{ label: "Later", value: false }, { label: u.canInstall ? "Update and restart" : "Download", value: true, primary: true }]);
  if (!go) return;
  if (!u.canInstall) return invoke("open_link", { url: u.page }).catch((e) => toast(errText(e), "err"));
  installUpdate();
}

async function installUpdate() {
  Upd.busy = true;
  Player.flushListen?.();
  const bar = h("div", { class: "update-bar" }, h("span", { class: "update-fill" }));
  const label = h("p", { class: "muted small" }, "Downloading…");
  const wrap = h("div", { class: "modal-wrap" }, h("div", { class: "modal", role: "dialog", "aria-modal": "true", "aria-label": "Updating Atmosphere" }, h("h2", {}, `Updating to ${Upd.info.version}`), bar, label));
  document.body.append(wrap);
  const off = await listen("atmosphere://update", (e) => {
    const { received, total } = e.payload || {};
    if (total) {
      bar.firstChild.style.width = `${Math.min(100, (received / total) * 100).toFixed(1)}%`;
      label.textContent = `${(received / 1048576).toFixed(1)} of ${(total / 1048576).toFixed(1)} MB`;
      if (received >= total) label.textContent = "Installing… Atmosphere will restart.";
    } else label.textContent = `${(received / 1048576).toFixed(1)} MB`;
  });
  try {
    await invoke("update_install");
  } catch (e) {
    off();
    wrap.remove();
    Upd.busy = false;
    toast(errText(e), "err");
  }
}

async function bindUpdates() {
  Upd.app = await invoke("app_version").catch(() => null);
  renderUpdate();
  $("#updateBtn")?.addEventListener("click", () => (Upd.info ? openUpdate() : checkUpdates(true)));
  $("#updateChip")?.addEventListener("click", openUpdate);
  const auto = () => state.settings?.checkUpdates !== false && !Upd.busy;
  if (auto()) setTimeout(() => checkUpdates(false), 5000);
  setInterval(() => { if (auto() && Date.now() - Upd.checkedAt >= UPDATE_EVERY) checkUpdates(false); }, 15 * 60 * 1000);
  window.addEventListener("focus", () => { if (auto() && Upd.checkedAt && Date.now() - Upd.checkedAt >= UPDATE_EVERY) checkUpdates(false); });
}
