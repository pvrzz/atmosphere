/* atmosphere · nowplaying.js · Player bar, effects panel (presets, EQ, filters, pitch, tempo), queue drawer and playback shortcuts. */
const LOG = (v, min, max) => Math.round(min * Math.pow(max / min, v / 100));
const UNLOG = (f, min, max) => Math.round((Math.log(f / min) / Math.log(max / min)) * 100);
const fmtHz = (f) => (f >= 1000 ? `${nf.format(f / 1000)}k` : String(f));

function rangeFill(el) {
  const p = ((el.value - el.min) / (el.max - el.min)) * 100;
  el.style.setProperty("--p", `${p}%`);
}

function updatePlayerBar() {
  const t = Player.current;
  $("#player").classList.toggle("idle", !t);
  $("#npArt").replaceChildren(t ? cover(t.art, "np-cover", t.album || t.title) : h("div", { class: "np-cover ph" }, icon("music")));
  $("#npTitle").textContent = t ? t.title : "Nothing playing";
  $("#npArtist").replaceChildren(t ? artistLinks(t) : h("span", { class: "muted" }, "Pick something from your library"));
  $("#npLike").replaceChildren(t ? likeBtn(t) : "");
  const paused = Player.audio.paused;
  document.body.classList.toggle("is-paused", paused || !t);
  $("#playBtn").innerHTML = svg(paused ? "player-play-filled" : "player-pause-filled");
  $("#playBtn").setAttribute("aria-label", paused ? "Play" : "Pause");
  $("#shuffleBtn").setAttribute("aria-pressed", String(Player.shuffle));
  $("#repeatBtn").setAttribute("aria-pressed", String(Player.repeat !== "off"));
  $("#repeatBtn").replaceChildren(icon(Player.repeat === "one" ? "repeat-once" : "repeat"));
  $("#repeatBtn").title = { off: "Repeat off", all: "Repeat all", one: "Repeat one" }[Player.repeat];
  $("#fxBtn").setAttribute("aria-pressed", String(fxActive()));
  document.title = t ? `${t.title} · ${t.artist || "Atmosphere"}` : "Atmosphere";
}

function updateTime() {
  const a = Player.audio;
  const dur = isFinite(a.duration) ? a.duration : (Player.current?.durationMs || 0) / 1000;
  $("#tCur").textContent = fmtDur(a.currentTime * 1000);
  $("#tDur").textContent = fmtDur(dur * 1000);
  const seek = $("#seek");
  if (!seek.matches(":active")) {
    seek.value = dur ? Math.round((a.currentTime / dur) * 1000) : 0;
    rangeFill(seek);
  }
}

function updateVolume() {
  const v = $("#volume");
  v.value = Math.round(Player.volume * 100);
  rangeFill(v);
  $("#muteBtn").replaceChildren(icon(Player.muted || Player.volume === 0 ? "volume-off" : "volume"));
}

function fxActive() {
  const f = Player.fx;
  return !Player.bypass && (f.tempo !== 1 || f.pitch !== 0 || f.reverb > 0 || f.wide8d || f.lowpass < 20000 || f.highpass > 20 || f.eq.some((g) => g !== 0));
}

/* effects panel */
const Fx = { tab: "presets" };

function userPresets() {
  return (state.settings?.fxPresets || []).filter((p) => p && p.id && p.fx);
}

function smoothPath(pts) {
  let d = `M${pts[0][0]},${pts[0][1]}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[i - 1] || pts[i], p1 = pts[i], p2 = pts[i + 1], p3 = pts[i + 2] || p2;
    const c1 = [p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6];
    const c2 = [p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6];
    d += ` C${c1[0].toFixed(1)},${c1[1].toFixed(1)} ${c2[0].toFixed(1)},${c2[1].toFixed(1)} ${p2[0].toFixed(1)},${p2[1].toFixed(1)}`;
  }
  return d;
}

function miniCurve(eq) {
  const W = 120, H = 34;
  const pts = (eq || EQ_BANDS.map(() => 0)).map((g, i) => [4 + (i * (W - 8)) / 9, H / 2 - (Math.max(-7, Math.min(7, g)) / 7) * (H / 2 - 3)]);
  const el = h("span", { class: "mini-curve", "aria-hidden": "true" });
  el.innerHTML = `<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none"><path class="zero" d="M0,${H / 2} H${W}"/><path class="line" d="${smoothPath(pts)}"/></svg>`;
  return el;
}

function presetTile(p, removable = false) {
  const on = Player.fx.preset === p.id && !Player.bypass;
  const tag = p.fx.tempo && p.fx.tempo !== 1 ? `${p.fx.tempo}×` : p.fx.wide8d ? "8D" : p.fx.reverb ? "Reverb" : null;
  const tile = h("button", { class: `preset ${on ? "on" : ""}`, "aria-pressed": String(on), onclick: () => { if (Player.bypass) Player.setBypass(false); Player.usePreset(p); renderFx(); } },
    miniCurve(p.fx.eq),
    h("span", { class: "preset-name" }, p.name, tag ? h("span", { class: "preset-tag" }, tag) : null),
    h("span", { class: "preset-desc" }, p.desc || "Your preset"));
  if (!removable) return tile;
  return h("div", { class: "preset-wrap" }, tile, h("button", { class: "icon-btn preset-x", "aria-label": `Delete ${p.name}`, onclick: async () => { state.settings = await invoke("save_settings", { patch: { fxPresets: userPresets().filter((x) => x.id !== p.id) } }); renderFx(); } }, icon("x")));
}

function eqEditor() {
  const W = 600, H = 170, PX = 28, PY = 16;
  const xs = EQ_BANDS.map((_, i) => PX + (i * (W - PX * 2)) / 9);
  const yOf = (g) => H / 2 - (g / 12) * (H / 2 - PY);
  const gOf = (y) => Math.max(-12, Math.min(12, Math.round((((H / 2 - y) / (H / 2 - PY)) * 12) * 2) / 2));
  const wrap = h("div", { class: "eq-editor" });
  const grid = [12, 6, 0, -6, -12].map((g) => `<path class="${g === 0 ? "zero" : "grid"}" d="M${PX - 8},${yOf(g)} H${W - PX + 8}"/><text x="${PX - 12}" y="${yOf(g) + 3.5}" text-anchor="end">${g > 0 ? "+" : ""}${g}</text>`).join("");
  wrap.innerHTML = `<svg viewBox="0 0 ${W} ${H + 22}" role="group" aria-label="Equalizer">${grid}<path class="area"/><path class="curve"/>${EQ_BANDS.map((f, i) => `<text class="hz" x="${xs[i]}" y="${H + 16}" text-anchor="middle">${fmtHz(f)}</text>`).join("")}${EQ_BANDS.map((f, i) => `<g class="node" data-i="${i}" tabindex="0" role="slider" aria-label="${fmtHz(f)} hertz" aria-valuemin="-12" aria-valuemax="12"><circle class="hit" r="16"/><circle class="dot" r="7"/><text class="val" y="-14" text-anchor="middle"></text></g>`).join("")}</svg>`;
  const svgEl = wrap.firstChild;
  const draw = () => {
    const eq = Player.fx.eq;
    const pts = eq.map((g, i) => [xs[i], yOf(g)]);
    const line = smoothPath(pts);
    svgEl.querySelector(".curve").setAttribute("d", line);
    svgEl.querySelector(".area").setAttribute("d", `${line} L${xs[9]},${H / 2} L${xs[0]},${H / 2} Z`);
    svgEl.querySelectorAll(".node").forEach((n, i) => {
      n.setAttribute("transform", `translate(${xs[i]},${yOf(eq[i])})`);
      n.setAttribute("aria-valuenow", eq[i]);
      n.querySelector(".val").textContent = eq[i] ? `${eq[i] > 0 ? "+" : ""}${eq[i]}` : "0";
    });
  };
  const set = (i, g) => {
    const eq = [...Player.fx.eq];
    eq[i] = g;
    if (Player.bypass) Player.setBypass(false);
    Player.applyFx({ eq }, "custom");
    draw();
  };
  svgEl.querySelectorAll(".node").forEach((n) => {
    const i = Number(n.dataset.i);
    n.addEventListener("pointerdown", (e) => {
      n.setPointerCapture(e.pointerId);
      n.classList.add("drag");
      const move = (ev) => {
        const r = svgEl.getBoundingClientRect();
        set(i, gOf(((ev.clientY - r.top) / r.height) * (H + 22)));
      };
      const up = () => { n.classList.remove("drag"); n.removeEventListener("pointermove", move); n.removeEventListener("pointerup", up); };
      n.addEventListener("pointermove", move);
      n.addEventListener("pointerup", up);
    });
    n.addEventListener("dblclick", () => set(i, 0));
    n.addEventListener("keydown", (e) => {
      const step = e.shiftKey ? 2 : 0.5;
      const g = Player.fx.eq[i];
      if (e.key === "ArrowUp") { e.preventDefault(); set(i, Math.min(12, g + step)); }
      if (e.key === "ArrowDown") { e.preventDefault(); set(i, Math.max(-12, g - step)); }
      if (e.key === "Home" || e.key === "0") { e.preventDefault(); set(i, 0); }
    });
  });
  draw();
  return wrap;
}

function fxRange(value, min, max, step, onInput) {
  const input = h("input", { type: "range", class: "range strong", min, max, step, value });
  rangeFill(input);
  input.addEventListener("input", () => { rangeFill(input); if (Player.bypass) Player.setBypass(false); onInput(Number(input.value)); });
  return input;
}

function adjustTab() {
  const fx = Player.fx;
  const tempoOut = h("output", { class: "big-val" }, `${fx.tempo.toFixed(2)}×`);
  const tempo = fxRange(fx.tempo, 0.5, 2, 0.01, (v) => { tempoOut.textContent = `${v.toFixed(2)}×`; Player.applyFx({ tempo: v }, "custom"); syncSteps(); });
  const steps = h("div", { class: "chips tight" }, ...[0.75, 1, 1.25, 1.5].map((v) => h("button", { class: "chip", "data-v": v, onclick: () => { Player.applyFx({ tempo: v }, "custom"); renderFx(); } }, v === 1 ? "Normal" : `${v}×`)));
  const syncSteps = () => $$(".chip", steps).forEach((c) => c.setAttribute("aria-pressed", String(Number(c.dataset.v) === Player.fx.tempo)));
  syncSteps();
  const pitchDisabled = fx.vinyl || (!!Player.ctx && !Player.pitchReady);
  const pitchVal = h("output", { class: "big-val" }, fx.vinyl ? "Follows speed" : `${fx.pitch > 0 ? "+" : ""}${fx.pitch} st`);
  const bump = (d) => { Player.applyFx({ pitch: Math.max(-12, Math.min(12, Player.fx.pitch + d)) }, "custom"); renderFx(); };
  const mode = h("div", { class: "seg wide", role: "radiogroup", "aria-label": "How speed affects pitch" },
    h("button", { type: "button", role: "radio", "aria-pressed": String(!fx.vinyl), onclick: () => { Player.applyFx({ vinyl: false }, "custom"); renderFx(); } }, "Keep pitch"),
    h("button", { type: "button", role: "radio", "aria-pressed": String(fx.vinyl), onclick: () => { Player.applyFx({ vinyl: true }, "custom"); renderFx(); } }, "Vinyl"));
  const wide = h("input", { type: "checkbox", class: "switch", "aria-label": "8D audio" });
  wide.checked = fx.wide8d;
  wide.addEventListener("change", () => Player.applyFx({ wide8d: wide.checked }, "custom"));
  const row = (label, control, value) => h("div", { class: "fx-row" }, h("span", { class: "fx-row-label" }, label), control, value ? h("output", { class: "fx-row-val" }, value) : null);
  const lpOut = h("output", { class: "fx-row-val" }, fx.lowpass >= 20000 ? "Off" : `${fmtHz(fx.lowpass)}Hz`);
  const hpOut = h("output", { class: "fx-row-val" }, fx.highpass <= 20 ? "Off" : `${fmtHz(fx.highpass)}Hz`);
  const rvOut = h("output", { class: "fx-row-val" }, `${Math.round(fx.reverb * 100)}%`);
  return h("div", { class: "fx-adjust" },
    h("section", { class: "fx-card" }, h("div", { class: "fx-card-head" }, h("h3", {}, "Equalizer"), h("span", { class: "muted small" }, "Drag the points. Double-click to reset one.")), eqEditor()),
    h("div", { class: "fx-split" },
      h("section", { class: "fx-card" },
        h("h3", {}, "Speed"),
        h("div", { class: "val-row" }, tempoOut, steps),
        tempo,
        h("h3", { class: "gap" }, "Pitch"),
        h("div", { class: "val-row" }, pitchVal, h("div", { class: "stepper" }, h("button", { type: "button", disabled: pitchDisabled, "aria-label": "Lower pitch", onclick: () => bump(-1) }, "−"), h("button", { type: "button", disabled: pitchDisabled || fx.pitch === 0, "aria-label": "Reset pitch", onclick: () => { Player.applyFx({ pitch: 0 }, "custom"); renderFx(); } }, "0"), h("button", { type: "button", disabled: pitchDisabled, "aria-label": "Raise pitch", onclick: () => bump(1) }, "+"))),
        mode,
        h("p", { class: "muted small" }, fx.vinyl ? "Speed changes pitch too, like a turntable." : !Player.pitchReady && Player.ctx ? "Pitch shifting isn't supported by this system's web engine." : "Change speed and pitch separately.")),
      h("section", { class: "fx-card" },
        h("h3", {}, "Space"),
        row("Reverb", fxRange(Math.round(fx.reverb * 100), 0, 100, 1, (v) => { rvOut.textContent = `${v}%`; Player.applyFx({ reverb: v / 100 }, "custom"); }), null), rvOut,
        h("label", { class: "fx-toggle" }, wide, h("span", {}, h("strong", {}, "8D audio"), h("small", {}, "Circles the sound around you. Best with headphones."))),
        h("h3", { class: "gap" }, "Filters"),
        row("High cut", fxRange(UNLOG(fx.lowpass, 400, 20000), 0, 100, 1, (v) => { const f = LOG(v, 400, 20000); lpOut.textContent = f >= 20000 ? "Off" : `${fmtHz(f)}Hz`; Player.applyFx({ lowpass: f }, "custom"); }), null), lpOut,
        row("Low cut", fxRange(UNLOG(fx.highpass, 20, 1200), 0, 100, 1, (v) => { const f = LOG(v, 20, 1200); hpOut.textContent = f <= 20 ? "Off" : `${fmtHz(f)}Hz`; Player.applyFx({ highpass: f }, "custom"); }), null), hpOut))
  );
}

function presetsTab() {
  const mine = userPresets();
  return h("div", { class: "fx-presets" },
    h("h3", {}, "Tone"), h("div", { class: "preset-grid" }, ...PRESETS.filter((p) => p.group === "Tone").map((p) => presetTile(p))),
    h("h3", {}, "Mood"), h("div", { class: "preset-grid" }, ...PRESETS.filter((p) => p.group === "Mood").map((p) => presetTile(p))),
    h("h3", {}, "Yours"), h("div", { class: "preset-grid" }, ...mine.map((p) => presetTile(p, true)), h("button", { class: "preset add", onclick: savePreset }, icon("plus"), h("span", { class: "preset-name" }, "Save current sound"), h("span", { class: "preset-desc" }, "Keep your tweaks as a preset"))));
}

function renderFx() {
  const panel = $("#fxPanel");
  if (panel.hidden) return;
  const y = panel.querySelector(".fx-scroll")?.scrollTop || 0;
  const onSw = h("input", { type: "checkbox", class: "switch", "aria-label": "Effects on" });
  onSw.checked = !Player.bypass;
  onSw.addEventListener("change", () => { Player.setBypass(!onSw.checked); renderFx(); });
  const tabs = h("div", { class: "seg", role: "tablist" }, ...[["presets", "Presets"], ["adjust", "Adjust"]].map(([id, label]) => h("button", { type: "button", role: "tab", "aria-pressed": String(Fx.tab === id), "aria-selected": String(Fx.tab === id), onclick: () => { Fx.tab = id; renderFx(); } }, label)));
  const current = Player.bypass ? "Effects off: original sound" : PRESETS.concat(userPresets()).find((p) => p.id === Player.fx.preset)?.name || (fxActive() ? "Custom" : "Flat");
  panel.replaceChildren(
    h("div", { class: "fx-head" },
      h("div", { class: "fx-title" }, h("h2", {}, "Sound"), h("span", { class: "muted small" }, current)),
      tabs,
      h("div", { class: "row" }, h("label", { class: "fx-power", title: "Compare with the original sound" }, h("span", {}, "Effects"), onSw), h("button", { class: "btn small ghost", onclick: () => { Player.resetFx(); renderFx(); } }, "Reset"), h("button", { class: "icon-btn", "aria-label": "Close", onclick: closeFx }, icon("x")))),
    h("div", { class: `fx-scroll ${Player.bypass ? "bypassed" : ""}` }, Fx.tab === "presets" ? presetsTab() : adjustTab())
  );
  panel.querySelector(".fx-scroll").scrollTop = y;
  $("#fxBtn").setAttribute("aria-pressed", String(fxActive()));
}

async function savePreset() {
  const input = h("input", { class: "text wide", placeholder: "Preset name", "aria-label": "Preset name" });
  const ok = await modal("Save sound preset", input, [{ label: "Cancel", value: false }, { label: "Save", value: true, primary: true }]);
  const name = input.value.trim();
  if (!ok || !name) return;
  const { preset, ...fx } = Player.fx;
  const id = `user-${Date.now()}`;
  const presets = [...userPresets(), { id, name, group: "Yours", desc: "Your preset", fx }];
  state.settings = await invoke("save_settings", { patch: { fxPresets: presets } });
  Player.applyFx({}, id);
  renderFx();
}

function openFx() {
  closeDrawers();
  $("#fxPanel").hidden = false;
  $("#fxBtn").setAttribute("aria-expanded", "true");
  renderFx();
}

function closeFx() {
  $("#fxPanel").hidden = true;
  $("#fxBtn").setAttribute("aria-expanded", "false");
}

/* queue drawer */
function renderQueue() {
  const panel = $("#queuePanel");
  if (panel.hidden) return;
  const now = Player.current;
  const next = Player.queue.slice(Player.index + 1);
  const row = (t, i, isNow) => h("div", { class: `q-row ${isNow ? "now" : ""}`, "data-qi": i, ondblclick: () => !isNow && Player.load(i) },
    cover(t.art, "lt-cover"), h("div", { class: "lt-main" }, h("span", { class: "lt-title" }, t.title), h("span", { class: "lt-sub" }, t.artist)),
    isNow ? h("span", { class: "flag" }, "Now") : h("button", { class: "icon-btn", "aria-label": `Remove ${t.title}`, onclick: () => Player.removeAt(i) }, icon("x")));
  panel.replaceChildren(
    h("div", { class: "drawer-head" }, h("h2", {}, "Queue"), h("div", { class: "row" }, next.length ? h("button", { class: "btn small ghost", onclick: () => Player.clearUpNext() }, "Clear") : null, h("button", { class: "icon-btn", "aria-label": "Close", onclick: closeDrawers }, icon("x")))),
    h("div", { class: "drawer-body" },
      now ? row(now, Player.index, true) : h("p", { class: "muted pad" }, "Nothing playing."),
      next.length ? h("h3", { class: "q-sub" }, `Up next · ${next.length}`) : null,
      ...next.slice(0, 300).map((t, k) => row(t, Player.index + 1 + k, false)))
  );
}

function closeDrawers() {
  $$(".drawer-panel").forEach((p) => (p.hidden = true));
  $$("#lyricsBtn, #queueBtn").forEach((b) => b.setAttribute("aria-pressed", "false"));
}

function toggleDrawer(id, btn, render) {
  const panel = $(id);
  const open = panel.hidden;
  closeDrawers();
  closeFx();
  panel.hidden = !open;
  $(btn).setAttribute("aria-pressed", String(open));
  if (open) render();
}

/* wiring */
function bindPlayer() {
  $("#playBtn").addEventListener("click", () => Player.toggle());
  $("#prevBtn").addEventListener("click", () => Player.prev());
  $("#nextBtn").addEventListener("click", () => Player.next());
  $("#shuffleBtn").addEventListener("click", () => Player.toggleShuffle());
  $("#repeatBtn").addEventListener("click", () => Player.cycleRepeat());
  $("#muteBtn").addEventListener("click", () => Player.toggleMute());
  $("#npTitle").addEventListener("click", () => {
    const t = Player.current;
    if (t?.album) openLib("album", `${lnorm(t.albumArtist || primaryArtist(t.artist))}|${lnorm(t.album)}`);
  });
  const seek = $("#seek");
  seek.addEventListener("input", () => {
    rangeFill(seek);
    const dur = Player.audio.duration;
    if (isFinite(dur)) $("#tCur").textContent = fmtDur((seek.value / 1000) * dur * 1000);
  });
  seek.addEventListener("change", () => Player.seek((seek.value / 1000) * Player.audio.duration));
  const vol = $("#volume");
  vol.addEventListener("input", () => { Player.muted = false; Player.setVolume(vol.value / 100); });
  $("#fxBtn").addEventListener("click", () => ($("#fxPanel").hidden ? openFx() : closeFx()));
  $("#queueBtn").addEventListener("click", () => toggleDrawer("#queuePanel", "#queueBtn", renderQueue));
  $("#lyricsBtn").addEventListener("click", () => toggleDrawer("#lyricsPanel", "#lyricsBtn", renderLyricsPanel));
  $("#tvBtn").addEventListener("click", (e) => chooseTvScreen(e.currentTarget));
  document.addEventListener("click", (e) => { if (e.target.isConnected && !$("#fxPanel").hidden && !e.target.closest("#fxPanel, #fxBtn, .modal-wrap")) closeFx(); });
  window.addEventListener("player", (e) => {
    const type = e.detail.type;
    if (type === "time") return updateTime();
    if (type === "volume") return updateVolume();
    if (type === "fx") return $("#fxBtn").setAttribute("aria-pressed", String(fxActive()));
    updatePlayerBar();
    updateTime();
    if (type === "queue" || type === "track") renderQueue();
  });
  document.addEventListener("keydown", (e) => {
    if (e.target.closest("input, textarea, select, [contenteditable]") || e.altKey) return;
    const mod = e.ctrlKey || e.metaKey;
    const k = e.key;
    if (k === " " && !mod) { e.preventDefault(); Player.toggle(); }
    else if (k === "ArrowRight" && mod) { e.preventDefault(); Player.next(); }
    else if (k === "ArrowLeft" && mod) { e.preventDefault(); Player.prev(); }
    else if (k === "ArrowRight" && !mod) Player.seek(Player.audio.currentTime + 5);
    else if (k === "ArrowLeft" && !mod) Player.seek(Player.audio.currentTime - 5);
    else if (k.toLowerCase() === "m" && !mod) Player.toggleMute();
    else if (k.toLowerCase() === "l" && !mod) $("#lyricsBtn").click();
    else if (k.toLowerCase() === "q" && !mod) $("#queueBtn").click();
    else if (k.toLowerCase() === "f" && !mod) openTv();
  });
  updatePlayerBar();
  updateTime();
  updateVolume();
}
