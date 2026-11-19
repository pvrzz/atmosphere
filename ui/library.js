/* atmosphere · library.js · Library views (songs, albums, artists, playlists, liked), context menu, dialogs and library/storage settings. */
const Lib = {
  data: null,
  byId: new Map(),
  songs: [],
  albums: new Map(),
  artists: new Map(),
  liked: new Set(),
  plays: {},
  route: { name: "home" },
  back: [],
  filter: "",
  sort: { key: "artist", dir: 1 },
  scan: null,
  drawSongs: null,
  sel: new Set(),
  anchor: -1,
  currentList: [],
};

const lnorm = (s) => (s || "").trim().toLowerCase();
function splitNames(s) {
  const text = (s || "").trim();
  let parts = text.split(/\s*(?:,|;|\s(?:feat\.?|ft\.?|featuring)\s|^(?:feat\.?|ft\.?)\s)\s*/i);
  if (/,/.test(text)) parts = parts.flatMap((p) => p.split(/\s+&\s+/));
  return parts.map((p) => p.trim()).filter(Boolean);
}
function titleCredits(title) {
  const t = title || "";
  const inBrackets = [...t.matchAll(/[\(\[]\s*(?:feat\.?|ft\.?|featuring|with)\s+([^\)\]]+)[\)\]]/gi)].map((m) => m[1]);
  const trailing = t.match(/\s(?:feat\.?|ft\.?|featuring)\s+([^\(\[]+)$/i);
  return [...inBrackets, trailing?.[1]].filter(Boolean).flatMap((x) => splitNames(x.replace(/,?\s+&\s+/g, ", ")));
}
const primaryArtist = (s) => splitNames(s)[0] || "Unknown artist";
const artistsOf = (t) => {
  const seen = new Map();
  for (const n of [t.albumArtist, ...splitNames(t.artist), ...titleCredits(t.title)]) {
    const k = lnorm(n);
    if (k && !seen.has(k)) seen.set(k, n.trim());
  }
  return [...seen.values()];
};
const sig = (t) => `${lnorm(t.title)}|${lnorm(primaryArtist(t.artist))}|${Math.round((t.durationMs || 0) / 2000)}`;
const albumKind = (a) => {
  const mins = a.duration / 60000;
  return a.tracks.length <= 3 && mins < 30 ? "Single" : a.tracks.length <= 6 && mins < 30 ? "EP" : "Album";
};

function buildIndexes(snap) {
  Lib.data = snap;
  Lib.byId = new Map(snap.tracks.map((t) => [t.id, t]));
  Lib.liked = new Set(snap.liked);
  Lib.plays = snap.plays || {};
  const main = new Set(snap.tracks.filter((t) => !t.playlist).map(sig));
  Lib.songs = snap.tracks.filter((t) => !t.playlist || !main.has(sig(t)));
  const albums = new Map();
  for (const t of Lib.songs) {
    if (!t.album) continue;
    const artist = t.albumArtist || primaryArtist(t.artist);
    const key = `${lnorm(artist)}|${lnorm(t.album)}`;
    let a = albums.get(key);
    if (!a) albums.set(key, (a = { key, title: t.album, artist, year: t.year, art: t.art, tracks: [], duration: 0 }));
    a.tracks.push(t);
    a.duration += t.durationMs || 0;
    a.art ||= t.art;
    a.year ||= t.year;
  }
  for (const a of albums.values()) a.tracks.sort((x, y) => (x.discNo || 1) - (y.discNo || 1) || (x.trackNo || 999) - (y.trackNo || 999) || x.title.localeCompare(y.title));
  Lib.albums = albums;
  const metaByName = new Map(Object.values(snap.artists || {}).map((m) => [lnorm(m.name), m]));
  const artists = new Map();
  const touch = (name) => {
    const k = lnorm(name);
    if (!artists.has(k)) artists.set(k, { key: k, name, meta: metaByName.get(k) || null, albums: [], tracks: [], appearsOn: [], featured: [] });
    return artists.get(k);
  };
  for (const a of albums.values()) touch(a.artist).albums.push(a);
  for (const t of Lib.songs) for (const n of artistsOf(t)) touch(n).tracks.push(t);
  for (const a of albums.values()) {
    for (const n of new Set(a.tracks.flatMap(artistsOf))) {
      const ar = artists.get(lnorm(n));
      if (ar && lnorm(a.artist) !== ar.key && !ar.appearsOn.includes(a)) ar.appearsOn.push(a);
    }
  }
  for (const a of albums.values()) {
    const byArtist = new Map();
    for (const t of a.tracks) {
      for (const n of artistsOf(t)) {
        const k = lnorm(n);
        if (k === lnorm(a.artist)) continue;
        if (!byArtist.has(k)) byArtist.set(k, []);
        byArtist.get(k).push(t);
      }
    }
    for (const [k, tracks] of byArtist) {
      const ar = artists.get(k);
      if (ar) ar.featured.push({ kind: albumKind(a), title: a.title, by: a.artist, art: a.art, year: a.year, on: tracks.map((t) => t.title), album: a });
    }
  }
  for (const t of Lib.songs) {
    if (t.album) continue;
    const main = lnorm(primaryArtist(t.artist));
    for (const n of artistsOf(t)) {
      const ar = artists.get(lnorm(n));
      if (ar && ar.key !== main) ar.featured.push({ kind: "Song", title: t.title, by: primaryArtist(t.artist), art: t.art, year: t.year, on: [], track: t });
    }
  }
  for (const ar of artists.values()) {
    ar.albums.sort((x, y) => (y.year || 0) - (x.year || 0));
    ar.featured.sort((x, y) => (y.year || 0) - (x.year || 0) || x.title.localeCompare(y.title));
  }
  Lib.artists = artists;
}

async function loadLibrary() {
  const snap = await invoke("library_get").catch(() => null);
  if (!snap) return;
  buildIndexes(snap);
  for (const id of [...Lib.sel]) if (!Lib.byId.has(id)) Lib.sel.delete(id);
  if (!Lib.restored) {
    Lib.restored = true;
    Player.restore((id) => Lib.byId.get(id));
    setTimeout(queueLibraryArtists, 4000);
    if (state.view === "get" && !state.collections.length) openLib("home", null, false);
  } else {
    Player.refreshTracks((id) => Lib.byId.get(id) || null);
  }
  renderPlaylistNav();
  if (state.view === "library") renderLib(true);
  if (typeof updateCommit === "function") updateCommit();
}

const artistImage = (ar) => ar?.meta?.image || ar?.albums.find((a) => a.art)?.art || ar?.tracks.find((t) => t.art)?.art || null;

function cover(key, cls, alt = "") {
  return art(artUrl(key), cls, alt);
}

/* routing */
function openLib(name, id = null, push = true) {
  if (push && state.view === "library") Lib.back.push(Lib.route);
  Lib.route = { name, id };
  Lib.filter = "";
  go("library");
  renderLib();
}

function libBack() {
  const prev = Lib.back.pop();
  if (prev) {
    Lib.route = prev;
    renderLib();
  }
}

function markLibNav() {
  const r = Lib.route;
  $$("[data-lib]").forEach((b) => (state.view === "library" && (b.dataset.lib === r.name || (r.name === "album" && b.dataset.lib === "albums") || (r.name === "artist" && b.dataset.lib === "artists")) ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current")));
  $$("#playlistNav .nav-item").forEach((b) => (state.view === "library" && r.name === "playlist" && b.dataset.playlist === r.id ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current")));
}

/* playback helpers */
function playList(list, i = 0, shuffle = null) {
  if (!list.length) return toast("Nothing to play here yet.", "err");
  Player.setQueue(list, i, shuffle);
}

/* track rows */
function likeBtn(t) {
  const on = Lib.liked.has(t.id);
  return h("button", { class: `icon-btn like ${on ? "on" : ""}`, "aria-label": on ? "Unlike" : "Like", "aria-pressed": String(on), onclick: (e) => { e.stopPropagation(); toggleLike(t); } }, icon(on ? "heart-filled" : "heart"));
}

async function toggleLike(t) {
  const on = !Lib.liked.has(t.id);
  on ? Lib.liked.add(t.id) : Lib.liked.delete(t.id);
  await invoke("track_like", { id: t.id, liked: on }).catch(() => {});
  $$(`.lt-row[data-id="${t.id}"] .like`).forEach((b) => b.replaceWith(likeBtn(t)));
  updatePlayerBar();
  if (Lib.route.name === "liked") renderLib(true);
}

function artistLinks(t) {
  const names = artistsOf({ ...t, albumArtist: "" });
  const wrap = h("span", { class: "links" });
  names.forEach((n, i) => {
    if (i) wrap.append(", ");
    wrap.append(h("button", { class: "link", onclick: (e) => { e.stopPropagation(); openLib("artist", lnorm(n)); } }, n));
  });
  return wrap;
}

function trackRow(t, i, ctx) {
  const playing = Player.current?.id === t.id;
  const label = ctx.numbers ? String(ctx.numbers(t, i)) : String(i + 1);
  const row = h("div", { class: `lt-row ${playing ? "playing" : ""} ${ctx.cover ? "with-cover" : ""} ${Lib.sel.has(t.id) ? "sel" : ""}`, role: "row", "aria-selected": String(Lib.sel.has(t.id)), "data-id": t.id, "data-i": i, "data-num": label, draggable: "true", tabindex: "-1",
    onclick: (e) => selectRow(e, t, i, ctx),
    ondblclick: () => playList(ctx.list, i),
    oncontextmenu: (e) => { e.preventDefault(); if (!Lib.sel.has(t.id)) selectRow({}, t, i, ctx); trackMenu(e, t, ctx, i); },
    onkeydown: (e) => { if (e.key === "Enter") playList(ctx.list, i); },
  },
    h("span", { class: "lt-num" }, playing ? eqBars() : label,
      h("button", { class: "lt-play", "aria-label": `Play ${t.title}`, onclick: (e) => { e.stopPropagation(); playList(ctx.list, i); } }, icon("player-play-filled"))),
    ctx.cover ? cover(t.art, "lt-cover") : null,
    h("div", { class: "lt-main" }, h("span", { class: "lt-title" }, t.title, t.video ? h("span", { class: "badge-video", title: "Has a music video" }, icon("video")) : null), ctx.hideArtist && !ctx.showArtistFor?.(t) ? null : h("span", { class: "lt-sub" }, artistLinks(t))),
    ctx.hideAlbum ? h("span") : h("span", { class: "lt-album" }, t.album ? h("button", { class: "link", onclick: (e) => { e.stopPropagation(); openLib("album", `${lnorm(t.albumArtist || primaryArtist(t.artist))}|${lnorm(t.album)}`); } }, t.album) : ""),
    likeBtn(t),
    h("span", { class: "lt-dur" }, fmtDur(t.durationMs)),
    ctx.playlist ? h("button", { class: "icon-btn", "aria-label": "Remove from playlist", onclick: (e) => { e.stopPropagation(); removeFromPlaylist(ctx.playlist, [t]); } }, icon("x")) : h("button", { class: "icon-btn", "aria-label": "More", onclick: (e) => { e.stopPropagation(); trackMenu(e, t, ctx, i); } }, icon("dots"))
  );
  row.addEventListener("dragstart", (e) => {
    const ids = Lib.sel.has(t.id) && Lib.sel.size > 1 ? [...Lib.sel] : [t.id];
    e.dataTransfer.setData("application/x-atmo-tracks", JSON.stringify(ids));
    e.dataTransfer.setData("text/plain", t.title);
    e.dataTransfer.effectAllowed = "copyMove";
  });
  return row;
}

const eqBars = () => h("span", { class: "eqbars", "aria-label": "Now playing" }, h("i"), h("i"), h("i"));

function selectRow(e, t, i, ctx) {
  Lib.currentList = ctx.list;
  if (e.shiftKey && Lib.anchor >= 0) {
    const [a, b] = [Math.min(Lib.anchor, i), Math.max(Lib.anchor, i)];
    if (!(e.ctrlKey || e.metaKey)) Lib.sel.clear();
    ctx.list.slice(a, b + 1).forEach((x) => Lib.sel.add(x.id));
  } else if (e.ctrlKey || e.metaKey) {
    Lib.sel.has(t.id) ? Lib.sel.delete(t.id) : Lib.sel.add(t.id);
    Lib.anchor = i;
  } else {
    Lib.sel = new Set([t.id]);
    Lib.anchor = i;
  }
  markSelection();
}

function markSelection() {
  $$(".lt-row").forEach((r) => {
    const on = Lib.sel.has(r.dataset.id);
    r.classList.toggle("sel", on);
    r.setAttribute("aria-selected", String(on));
  });
}

function selectedTracks(fallback) {
  const picked = [...Lib.sel].map((id) => Lib.byId.get(id)).filter(Boolean);
  return picked.length && (!fallback || Lib.sel.has(fallback.id)) ? picked : fallback ? [fallback] : [];
}

function playlistsContaining(t) {
  const key = sig(t);
  return (Lib.data?.playlists || []).map((p) => ({ p, copies: p.order.map((id) => Lib.byId.get(id)).filter((x) => x && sig(x) === key) })).filter((x) => x.copies.length);
}

const songs = (n) => (n === 1 ? "song" : `${n} songs`);

async function deleteTracks(tracks) {
  if (!tracks.length) return;
  const inPlaylists = tracks.some((t) => !t.playlist && playlistsContaining(t).length);
  const box = h("input", { type: "checkbox", class: "box" });
  box.checked = true;
  const body = h("div", { class: "stack" },
    h("p", { class: "muted" }, `${tracks.length === 1 ? `“${tracks[0].title}”` : songs(tracks.length)} will be moved to your ${isMac ? "Trash" : "Recycle Bin"}, along with any lyrics files, so you can restore them from there if you change your mind.`),
    inPlaylists ? h("label", { class: "check-row" }, box, h("span", {}, "Also remove them from your playlists")) : null);
  const ok = await modal(tracks.length === 1 ? "Delete this song from your device?" : `Delete ${tracks.length} songs from your device?`, body, [{ label: "Cancel", value: false }, { label: "Delete", value: true, primary: true }]);
  if (!ok) return;
  try {
    const n = await invoke("tracks_delete", { ids: tracks.map((t) => t.id), playlists: inPlaylists && box.checked });
    Lib.sel.clear();
    toast(`Deleted ${songs(n)}. ${isMac ? "Trash" : "Recycle Bin"} has them if you need them back.`, "ok");
  } catch (e) {
    toast(errText(e), "err");
  }
}

async function hideTracks(tracks) {
  if (!tracks.length) return;
  const n = await invoke("tracks_hide", { ids: tracks.map((t) => t.id) }).catch((e) => (toast(errText(e), "err"), 0));
  Lib.sel.clear();
  if (n) toast(`Removed ${songs(n)} from your library. The files are still on your drive; restore them in Settings → Library.`, "ok");
}

function trackList(list, ctx = {}) {
  ctx.list = ctx.list || list;
  const wrap = h("div", { class: "lt", role: "table" });
  list.forEach((t, i) => {
    const at = ctx.list.indexOf(t);
    wrap.append(trackRow(t, at >= 0 ? at : i, ctx));
  });
  if (ctx.playlist) enableReorder(wrap, ctx.playlist);
  return wrap;
}

function markPlaying() {
  const id = Player.current?.id;
  $$(".lt-row").forEach((r) => {
    const on = r.dataset.id === id;
    if (on === r.classList.contains("playing")) return;
    r.classList.toggle("playing", on);
    const num = r.querySelector(".lt-num");
    const play = num.querySelector(".lt-play");
    num.replaceChildren(on ? eqBars() : r.dataset.num, play);
  });
}

/* context menu */
function closeMenus() {
  $$(".ctx").forEach((m) => m.remove());
}

function menuAt(x, y, items) {
  closeMenus();
  const m = h("div", { class: "menu ctx", role: "menu" });
  for (const it of items) {
    if (it === "-") m.append(h("div", { class: "menu-divider" }));
    else if (it) m.append(h("button", { role: "menuitem", class: `${it.sub ? "has-sub" : ""} ${it.danger ? "danger" : ""}`, disabled: it.disabled, onclick: (e) => { if (it.sub) { e.stopPropagation(); const r = e.currentTarget.getBoundingClientRect(); menuAt(r.right + 4, r.top - 6, it.sub()); } else { closeMenus(); it.run(); } } }, h("span", {}, it.label), it.sub ? icon("chevron-down") : it.hint ? h("kbd", {}, it.hint) : null));
  }
  document.body.append(m);
  const w = m.offsetWidth, hgt = m.offsetHeight;
  m.style.left = `${Math.min(x, innerWidth - w - 8)}px`;
  m.style.top = `${Math.min(y, innerHeight - hgt - 8)}px`;
  m.querySelector("button")?.focus();
  return m;
}

document.addEventListener("click", (e) => { if (!e.target.closest(".ctx")) closeMenus(); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeMenus(); });

function playlistItems(tracks) {
  return () => [
    ...(Lib.data?.playlists || []).map((p) => ({ label: p.name, run: () => addToPlaylist(p.name, tracks) })),
    (Lib.data?.playlists || []).length ? "-" : null,
    { label: "New playlist…", run: () => newPlaylist(tracks) },
  ];
}

function trackMenu(e, t, ctx, i) {
  const albumKey = t.album ? `${lnorm(t.albumArtist || primaryArtist(t.artist))}|${lnorm(t.album)}` : null;
  const many = selectedTracks(t);
  if (many.length > 1) {
    const inList = ctx.playlist ? many.filter((x) => x.playlist === ctx.playlist) : [];
    return menuAt(e.clientX, e.clientY, [
      { label: `Play ${many.length} songs`, run: () => playList(many, 0, false) },
      { label: "Play next", run: () => Player.playNext(many) },
      { label: "Add to queue", run: () => Player.addToQueue(many) },
      "-",
      { label: "Add to playlist", sub: playlistItems(many) },
      { label: `Edit info for ${many.length} songs…`, run: () => editTracks(many) },
      inList.length ? { label: `Remove ${inList.length} from this playlist`, run: () => removeFromPlaylist(ctx.playlist, inList) } : null,
      "-",
      { label: `Remove ${many.length} from library`, run: () => hideTracks(many) },
      { label: `Delete ${many.length} from device…`, danger: true, run: () => deleteTracks(many) },
    ]);
  }
  const holders = playlistsContaining(t).filter((x) => x.p.name !== ctx.playlist);
  menuAt(e.clientX, e.clientY, [
    { label: "Play", run: () => playList(ctx.list, i) },
    { label: "Play next", run: () => Player.playNext([t]) },
    { label: "Add to queue", run: () => Player.addToQueue([t]) },
    "-",
    { label: "Add to playlist", sub: playlistItems([t]) },
    { label: Lib.liked.has(t.id) ? "Unlike" : "Like", run: () => toggleLike(t) },
    "-",
    albumKey && { label: "Go to album", run: () => openLib("album", albumKey) },
    { label: "Go to artist", run: () => openLib("artist", lnorm(t.albumArtist || primaryArtist(t.artist))) },
    "-",
    { label: "Song info…", run: () => songInfo(t) },
    { label: "Edit info…", run: () => editInfo(t) },
    { label: "Show lyrics", run: () => { if (Player.current?.id !== t.id) playList(ctx.list, i); openLyrics(); } },
    { label: "Show in folder", run: () => invoke("reveal", { path: t.path }) },
    "-",
    ctx.playlist && t.playlist === ctx.playlist ? { label: "Remove from this playlist", run: () => removeFromPlaylist(ctx.playlist, [t]) } : null,
    holders.length ? { label: "Remove from playlist", sub: () => holders.map(({ p, copies }) => ({ label: p.name, run: () => removeFromPlaylist(p.name, copies) })) } : null,
    t.playlist ? null : { label: "Remove from library", run: () => hideTracks([t]) },
    { label: t.playlist ? "Delete this copy from device…" : "Delete from device…", danger: true, run: () => deleteTracks([t]) },
  ]);
}

/* playlists */
function renderPlaylistNav() {
  const nav = $("#playlistNav");
  const lists = Lib.data?.playlists || [];
  nav.replaceChildren(
    ...lists.map((p) => {
      const b = h("button", { class: `nav-item small ${p.missing ? "missing" : ""}`, "data-playlist": p.name, onclick: () => openLib("playlist", p.name) }, icon(p.missing ? "alert-circle" : p.linked ? "folder" : "playlist"), h("span", { class: "nav-label" }, p.name), h("span", { class: "nav-meta" }, p.missing ? "Missing" : String(p.order.length)));
      b.addEventListener("dragover", (e) => { if (e.dataTransfer.types.includes("application/x-atmo-tracks")) { e.preventDefault(); b.classList.add("drop"); } });
      b.addEventListener("dragleave", () => b.classList.remove("drop"));
      b.addEventListener("drop", (e) => {
        e.preventDefault();
        b.classList.remove("drop");
        const ids = JSON.parse(e.dataTransfer.getData("application/x-atmo-tracks") || "[]");
        addToPlaylist(p.name, ids.map((id) => Lib.byId.get(id)).filter(Boolean));
      });
      return b;
    })
  );
  $("#playlistEmpty").hidden = lists.length > 0;
  markLibNav();
}

async function addToPlaylist(name, tracks) {
  if (!tracks.length) return;
  try {
    const n = await invoke("playlist_add", { name, ids: tracks.map((t) => t.id) });
    toast(n ? `Added ${plural(n, "song")} to ${name}` : `Already in ${name}`, "ok");
  } catch (e) {
    toast(errText(e), "err");
  }
}

async function removeFromPlaylist(name, tracks) {
  const pl = (Lib.data?.playlists || []).find((x) => x.name === name);
  const bin = isMac ? "Trash" : "Recycle Bin";
  const ok = pl?.linked
    ? await confirmBox(`Remove ${tracks.length === 1 ? `“${tracks[0].title}”` : plural(tracks.length, "song")} from ${name}?`, `This is your own folder, so the file itself moves from it to your ${bin}. If it's the only copy, it leaves your library too. You can restore it from the ${bin}.`, "Move to " + bin)
    : await confirmBox(`Remove ${tracks.length === 1 ? `“${tracks[0].title}”` : plural(tracks.length, "song")} from ${name}?`, `The playlist's copy goes to your ${isMac ? "Trash" : "Recycle Bin"}. The song stays in your library.`, "Remove");
  if (!ok) return;
  try {
    await invoke("playlist_remove", { name, ids: tracks.map((t) => t.id) });
  } catch (e) {
    toast(errText(e), "err");
  }
}

function enableReorder(wrap, name) {
  let dragging = null;
  wrap.addEventListener("dragstart", (e) => (dragging = e.target.closest(".lt-row")));
  wrap.addEventListener("dragover", (e) => {
    const over = e.target.closest(".lt-row");
    if (!dragging || !over || over === dragging) return;
    e.preventDefault();
    const r = over.getBoundingClientRect();
    over.parentNode.insertBefore(dragging, e.clientY < r.top + r.height / 2 ? over : over.nextSibling);
  });
  wrap.addEventListener("dragend", async () => {
    if (!dragging) return;
    dragging = null;
    const ids = $$(".lt-row", wrap).map((r) => r.dataset.id);
    await invoke("playlist_reorder", { name, ids }).catch((e) => toast(errText(e), "err"));
    loadLibrary();
  });
}

/* dialogs */
function modal(title, body, actions) {
  return new Promise((resolve) => {
    const close = (v) => { wrap.remove(); resolve(v); };
    const wrap = h("div", { class: "modal-wrap", onclick: (e) => { if (e.target === wrap) close(null); } },
      h("div", { class: "modal", role: "dialog", "aria-modal": "true", "aria-label": title },
        h("h2", {}, title), body,
        h("div", { class: "row end" }, ...actions.map((a) => h("button", { class: `btn ${a.primary ? "primary" : a.danger ? "danger-ghost" : "ghost"}`, onclick: () => close(a.value) }, a.label)))));
    wrap.addEventListener("keydown", (e) => {
      if (e.key === "Escape") close(null);
      if (e.key === "Enter" && e.target.tagName === "INPUT") close(actions.find((a) => a.primary)?.value);
    });
    document.body.append(wrap);
    (wrap.querySelector("input, textarea") || wrap.querySelector(".btn.primary"))?.focus();
  });
}

const confirmBox = (title, text, label) => modal(title, h("p", { class: "muted" }, text), [{ label: "Cancel", value: false }, { label, value: true, primary: true }]);

async function linkFolder() {
  try {
    const name = await invoke("playlist_link");
    if (!name) return;
    toast(`Added “${name}” as a playlist. Atmosphere won't move or delete the folder.`, "ok");
    await loadLibrary();
    openLib("playlist", name);
  } catch (e) {
    toast(errText(e), "err");
  }
}

async function relocateFolder(name) {
  try {
    if (await invoke("playlist_relocate", { name })) {
      toast(`Found “${name}” again`, "ok");
      await loadLibrary();
    }
  } catch (e) {
    toast(errText(e), "err");
  }
}

async function unlinkFolder(name) {
  const ok = await confirmBox(`Remove “${name}” from Atmosphere?`, "The folder and every song in it stay exactly where they are on your device. Atmosphere just stops showing it as a playlist.", "Remove");
  if (!ok) return;
  try {
    await invoke("playlist_unlink", { name });
    openLib("songs", null, false);
  } catch (e) {
    toast(errText(e), "err");
  }
}

function playlistAddMenu(e) {
  e.stopPropagation();
  const r = e.currentTarget.getBoundingClientRect();
  menuAt(r.left, r.bottom + 4, [
    { label: "New playlist", run: () => newPlaylist() },
    { label: "Add existing folder…", run: linkFolder },
  ]);
}

async function newPlaylist(tracks = []) {
  const input = h("input", { class: "text wide", placeholder: "Playlist name", "aria-label": "Playlist name" });
  const ok = await modal("New playlist", h("div", { class: "stack" }, input, h("p", { class: "muted small" }, "It's a folder of songs on this device. Phones, USB drives and other players can use it as-is.")), [{ label: "Cancel", value: false }, { label: "Create", value: true, primary: true }]);
  const name = input.value.trim();
  if (!ok || !name) return;
  try {
    const created = await invoke("playlist_create", { name });
    if (tracks.length) await addToPlaylist(created, tracks);
    openLib("playlist", created);
  } catch (e) {
    toast(errText(e), "err");
  }
}

function shared(tracks, get) {
  const vals = new Set(tracks.map((t) => String(get(t) ?? "").trim()));
  return vals.size === 1 ? [...vals][0] : "";
}

async function editTracks(tracks, opts = {}) {
  if (!tracks.length) return;
  const title = opts.title || `Edit ${plural(tracks.length, "song")}`;
  const fields = [
    ["album", "Album", shared(tracks, (t) => t.album)],
    ["albumArtist", "Album artist", shared(tracks, (t) => t.albumArtist || (opts.album ? opts.album.artist : ""))],
    ["artist", "Artist", opts.album ? "" : shared(tracks, (t) => t.artist)],
    ["date", "Year", shared(tracks, (t) => t.year)],
    ["genre", "Genre", shared(tracks, (t) => t.genre)],
  ].filter(([k]) => !(opts.album && k === "artist"));
  const inputs = {};
  const grid = h("div", { class: "fields two" }, ...fields.map(([k, label, v]) => h("label", { class: `field ${k === "album" ? "full" : ""}` }, h("span", {}, label), (inputs[k] = h("input", { value: v, placeholder: v ? "" : "Mixed: leave blank to keep each song's own", spellcheck: "false" })))));
  let artwork = { kind: "keep" };
  const currentArt = shared(tracks, (t) => t.art);
  const preview = h("div", { class: "edit-cover" });
  const showPreview = (src) => preview.replaceChildren(src ? h("img", { src, alt: "" }) : h("div", { class: "edit-cover-ph" }, icon("photo")));
  showPreview(currentArt ? artUrl(currentArt) : null);
  const coverNote = h("p", { class: "muted small" }, currentArt ? "Current cover." : tracks.some((t) => t.art) ? "These songs have different covers." : "No cover yet.");
  const coverBox = h("div", { class: "edit-cover-row" }, preview, h("div", { class: "stack" },
    h("strong", {}, "Cover art"), coverNote,
    h("div", { class: "row" },
      h("button", { class: "btn small", type: "button", onclick: async () => { const p = await invoke("pick_image").catch((x) => (toast(errText(x), "err"), null)); if (p) { artwork = { kind: "file", path: p.path }; showPreview(p.dataUrl); coverNote.textContent = `New cover for all ${plural(tracks.length, "song")}.`; } } }, "Choose image…"),
      h("button", { class: "btn small ghost", type: "button", onclick: () => { artwork = { kind: "none" }; showPreview(null); coverNote.textContent = "The cover will be removed from every song."; } }, "Remove"))));
  const ok = await modal(title, h("div", { class: "stack" }, coverBox, grid, h("p", { class: "muted small" }, "Changes are written into each file's tags. Fields you leave untouched aren't changed.")), [{ label: "Cancel", value: false }, { label: "Save", value: true, primary: true }]);
  if (!ok) return;
  const meta = { artwork };
  for (const [k, , v] of fields) {
    const nv = inputs[k].value.trim();
    if (nv && nv !== v) meta[k] = nv;
  }
  if (Object.keys(meta).length === 1 && artwork.kind === "keep") return;
  toast(`Updating ${plural(tracks.length, "song")}…`, "");
  try {
    const n = await invoke("tracks_edit", { ids: tracks.map((t) => t.id), meta });
    toast(`Updated ${plural(n, "song")}`, "ok");
    await loadLibrary();
    if (opts.album) {
      const artist = meta.albumArtist || opts.album.artist;
      const key = `${lnorm(artist)}|${lnorm(meta.album || opts.album.title)}`;
      if (Lib.albums.has(key)) openLib("album", key, false);
    }
  } catch (e) {
    toast(errText(e), "err");
  }
}

const kindWord = (a) => (albumKind(a) === "EP" ? "EP" : albumKind(a).toLowerCase());
const editAlbum = (a) => editTracks(a.tracks, { album: a, title: `Edit ${kindWord(a)}` });

async function editPlaylist(p) {
  const input = h("input", { class: "text wide", value: p.name, "aria-label": "Playlist name", spellcheck: "false" });
  let cover = null;
  const preview = h("div", { class: "edit-cover" });
  const tracks = p.order.map((id) => Lib.byId.get(id)).filter(Boolean);
  const showPreview = (src) => preview.replaceChildren(src ? h("img", { src, alt: "" }) : h("div", { class: "edit-cover-ph" }, icon("playlist")));
  showPreview(p.cover ? artUrl(p.cover) : tracks[0]?.art ? artUrl(tracks[0].art) : null);
  const note = h("p", { class: "muted small" }, p.cover ? "Saved as cover.jpg in the playlist folder." : "Uses the songs' covers until you choose one.");
  const body = h("div", { class: "stack" },
    h("label", { class: "field full" }, h("span", {}, "Name"), input),
    h("div", { class: "edit-cover-row" }, preview, h("div", { class: "stack" }, h("strong", {}, "Playlist cover"), note,
      h("div", { class: "row" },
        h("button", { class: "btn small", type: "button", onclick: async () => { const x = await invoke("pick_image").catch((e) => (toast(errText(e), "err"), null)); if (x) { cover = { path: x.path }; showPreview(x.dataUrl); note.textContent = "Saved as cover.jpg in the folder, so phones and other players show it too."; } } }, "Choose image…"),
        p.cover ? h("button", { class: "btn small ghost", type: "button", onclick: () => { cover = { path: null }; showPreview(tracks[0]?.art ? artUrl(tracks[0].art) : null); note.textContent = "The cover image will go to the Recycle Bin."; } }, "Remove") : null))));
  const ok = await modal(p.linked ? "Edit linked folder" : "Edit playlist", body, [{ label: "Cancel", value: false }, { label: "Save", value: true, primary: true }]);
  if (!ok) return;
  let name = p.name;
  try {
    const want = input.value.trim();
    if (want && want !== p.name) name = await invoke("playlist_rename", { from: p.name, to: want });
    if (cover) await invoke("playlist_cover", { name, path: cover.path });
    await loadLibrary();
    openLib("playlist", name, false);
  } catch (e) {
    toast(errText(e), "err");
  }
}

async function editInfo(t) {
  const fields = [["title", "Title", t.title], ["artist", "Artist", t.artist], ["album", "Album", t.album], ["albumArtist", "Album artist", t.albumArtist], ["trackNumber", "Track", t.trackNo ?? ""], ["discNumber", "Disc", t.discNo ?? ""], ["date", "Year", t.year ?? ""], ["genre", "Genre", t.genre]];
  const inputs = {};
  const grid = h("div", { class: "fields two" }, ...fields.map(([k, label, v]) => h("label", { class: `field ${["title", "artist"].includes(k) ? "full" : ""}` }, h("span", {}, label), (inputs[k] = h("input", { value: String(v ?? ""), spellcheck: "false" })))));
  const ok = await modal("Edit info", h("div", { class: "stack" }, grid, h("p", { class: "muted small" }, "Changes are written into the file's tags, so they're kept everywhere you use it. Moving a song to a different album or artist is as simple as changing it here.")), [{ label: "Cancel", value: false }, { label: "Save", value: true, primary: true }]);
  if (!ok) return;
  const num = (v) => (v.trim() === "" ? null : parseInt(v, 10) || null);
  const meta = { title: inputs.title.value, artist: inputs.artist.value, album: inputs.album.value, albumArtist: inputs.albumArtist.value, trackNumber: num(inputs.trackNumber.value), discNumber: num(inputs.discNumber.value), date: inputs.date.value, genre: inputs.genre.value, artwork: { kind: "keep" } };
  try {
    await invoke("track_edit", { id: t.id, meta });
    toast("Saved to the file's tags", "ok");
  } catch (e) {
    toast(errText(e), "err");
  }
}

const ArtistFetch = { queue: [], busy: false, asked: new Set() };

function setArtistMeta(name, meta) {
  if (!meta) return;
  const ar = Lib.artists.get(lnorm(name));
  if (ar) ar.meta = meta;
  if (Lib.data) Lib.data.artists = { ...(Lib.data.artists || {}), [`n:${lnorm(name)}`]: meta };
  const r = Lib.route;
  if (state.view === "library" && ((r.name === "artist" && r.id === lnorm(name)) || r.name === "artists" || r.name === "home")) renderLib(true);
}

async function pumpArtistInfo() {
  if (ArtistFetch.busy || state.settings?.artistInfo === false) return;
  const name = ArtistFetch.queue.shift();
  if (!name) return;
  ArtistFetch.busy = true;
  try {
    setArtistMeta(name, await invoke("artist_fetch", { name, force: false }));
  } catch {}
  ArtistFetch.busy = false;
  if (ArtistFetch.queue.length) setTimeout(pumpArtistInfo, 1200);
}

function queueArtistInfo(names, first = false) {
  for (const n of names) {
    const k = lnorm(n);
    if (!k || k === "unknown artist" || ArtistFetch.asked.has(k)) continue;
    ArtistFetch.asked.add(k);
    first ? ArtistFetch.queue.unshift(n) : ArtistFetch.queue.push(n);
  }
  pumpArtistInfo();
}

function queueLibraryArtists() {
  const pending = [...Lib.artists.values()].filter((a) => !a.meta?.checked).sort((a, b) => b.tracks.length - a.tracks.length).slice(0, 300);
  queueArtistInfo(pending.map((a) => a.name));
}

async function searchArtistOnline(ar) {
  let pick = { photo: null, page: null };
  const input = h("input", { class: "text wide", type: "search", value: ar.name, "aria-label": "Search for this artist", spellcheck: "false" });
  const photos = h("div", { class: "pick-photos" });
  const pages = h("div", { class: "pick-pages" });
  const status = h("p", { class: "muted small" }, "Searching…");
  const fans = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M fans` : n >= 1e3 ? `${Math.round(n / 1e3)}K fans` : `${n} fans`);
  const run = async () => {
    status.textContent = "Searching…";
    pick = { photo: null, page: null };
    const r = await invoke("artist_choices", { query: input.value }).catch((e) => (status.textContent = errText(e), null));
    if (!r) return;
    photos.replaceChildren(...r.photos.map((p) => {
      const el = h("button", { class: "pick-photo", type: "button", title: `${p.name} · ${fans(p.fans)}`, onclick: () => { pick.photo = pick.photo === p.full ? null : p.full; $$(".pick-photo", photos).forEach((x) => x.classList.toggle("on", x === el && !!pick.photo)); } },
        h("img", { src: p.thumb, alt: "", referrerpolicy: "no-referrer", loading: "lazy" }), h("span", {}, p.name), h("small", {}, fans(p.fans)));
      return el;
    }));
    pages.replaceChildren(...r.pages.map((p) => {
      const el = h("button", { class: `pick-page ${p.music ? "" : "dim"}`, type: "button", onclick: () => { pick.page = pick.page === p.key ? null : p.key; $$(".pick-page", pages).forEach((x) => x.classList.toggle("on", x === el && !!pick.page)); } },
        h("strong", {}, p.title), h("span", {}, p.description || "No description"));
      return el;
    }));
    status.textContent = r.photos.length || r.pages.length ? "Pick a photo, a Wikipedia page for the bio and logo, or both." : "Nothing found. Try a different spelling.";
  };
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); run(); } });
  const body = h("div", { class: "stack" },
    h("div", { class: "row" }, input, h("button", { class: "btn", type: "button", onclick: run }, icon("search"), "Search")),
    status,
    h("strong", { class: "pick-head" }, "Photos from Deezer"), photos,
    h("strong", { class: "pick-head" }, "Wikipedia page (bio and logo)"), pages);
  run();
  const ok = await modal(`Find ${ar.name} online`, body, [{ label: "Cancel", value: false }, { label: "Use these", value: true, primary: true }]);
  if (!ok || (!pick.photo && !pick.page)) return null;
  try {
    const meta = await invoke("artist_apply", { name: ar.name, photo: pick.photo, page: pick.page });
    setArtistMeta(ar.name, meta);
    toast("Artist updated", "ok");
    return meta;
  } catch (e) {
    toast(errText(e), "err");
    return null;
  }
}

async function editArtist(ar) {
  let meta = ar.meta || {};
  const bio = h("textarea", { class: "text area", rows: "6", placeholder: "A few lines about this artist…" }, meta.bio || "");
  const photo = h("div", { class: "edit-cover round" });
  const logo = h("div", { class: "edit-logo" });
  const draw = () => {
    photo.replaceChildren(artistImage({ ...ar, meta }) ? h("img", { src: artUrl(artistImage({ ...ar, meta })), alt: "" }) : h("div", { class: "edit-cover-ph" }, icon("user")));
    logo.replaceChildren(meta.logo ? h("img", { class: `artist-logo ${meta.logoDark ? "dark" : ""}`, src: artUrl(meta.logo), alt: "" }) : h("span", { class: "muted small" }, "No logo. The name is shown as text."));
  };
  const act = (kind, pick) => async () => {
    try {
      meta = await invoke("artist_media", { name: ar.name, kind, pick });
      setArtistMeta(ar.name, meta);
      draw();
    } catch (e) {
      if (errText(e) !== "cancelled") toast(errText(e), "err");
    }
  };
  const pickPhoto = async () => {
    try {
      meta = await invoke("artist_update", { name: ar.name, pickImage: true, bio: null });
      setArtistMeta(ar.name, meta);
      draw();
    } catch (e) {
      if (errText(e) !== "cancelled") toast(errText(e), "err");
    }
  };
  const search = h("button", { class: "btn small", type: "button", onclick: async () => {
    const m = await searchArtistOnline(ar);
    if (m) {
      meta = m;
      bio.value = m.bio || "";
      draw();
    }
  } }, icon("search"), "Search online…");
  const refresh = h("button", { class: "btn small ghost", type: "button", onclick: async (e) => {
    const b = e.currentTarget;
    b.disabled = true;
    b.textContent = "Looking it up…";
    try {
      const m = await invoke("artist_fetch", { name: ar.name, force: true });
      if (m) {
        meta = m;
        bio.value = m.bio || "";
        setArtistMeta(ar.name, m);
        draw();
        toast(m.image || m.bio || m.logo ? "Updated from Deezer and Wikipedia" : "Nothing found online for this name", m.image || m.bio || m.logo ? "ok" : "");
      }
    } catch (x) {
      toast(errText(x), "err");
    }
    b.disabled = false;
    b.textContent = "Look up online again";
  } }, "Look up online again");
  draw();
  const body = h("div", { class: "stack" },
    h("div", { class: "edit-cover-row" }, photo, h("div", { class: "stack" }, h("strong", {}, "Photo"),
      h("div", { class: "row" }, h("button", { class: "btn small", type: "button", onclick: pickPhoto }, "Choose image…"), h("button", { class: "btn small ghost", type: "button", onclick: act("photo", false) }, "Remove")))),
    h("div", { class: "stack" }, h("strong", {}, "Logo"), logo,
      h("div", { class: "row" }, h("button", { class: "btn small", type: "button", onclick: act("logo", true) }, "Choose logo…"), h("button", { class: "btn small ghost", type: "button", onclick: act("logo", false) }, "Remove"))),
    h("label", { class: "field" }, h("span", {}, "About"), bio),
    h("div", { class: "row between" }, h("p", { class: "muted small" }, "Photos come from Deezer and bios and logos from Wikipedia. Not the right artist? Search online and pick, or use your own images. Anything you change here stays."), h("div", { class: "row" }, search, refresh)));
  const ok = await modal(`Edit ${ar.name}`, body, [{ label: "Cancel", value: false }, { label: "Save", value: true, primary: true }]);
  if (ok && bio.value !== (meta.bio || "")) {
    try {
      setArtistMeta(ar.name, await invoke("artist_update", { name: ar.name, pickImage: false, bio: bio.value }));
    } catch (e) {
      toast(errText(e), "err");
    }
  }
}

/* views */
function libHead(title, sub, actions = [], opts = {}) {
  const search = opts.search ? h("input", { class: "text lib-search", type: "search", placeholder: `Search ${title.toLowerCase()}`, value: Lib.filter, "aria-label": `Search ${title}`, oninput: (e) => { Lib.filter = e.target.value; opts.onSearch?.(); } }) : null;
  return h("header", { class: "lib-head" },
    Lib.back.length ? h("button", { class: "icon-btn back", "aria-label": "Back", onclick: libBack }, icon("arrow-left")) : null,
    h("div", { class: "lib-titles" }, h("h1", {}, title), sub ? h("p", { class: "view-sub" }, sub) : null),
    h("div", { class: "row" }, search, ...actions));
}

const playBtns = (list) => [
  h("button", { class: "btn primary", disabled: !list.length, onclick: () => playList(list, 0, false) }, icon("player-play-filled"), "Play"),
  h("button", { class: "btn", disabled: !list.length, onclick: () => playList(list, Math.floor(Math.random() * list.length), true) }, icon("arrows-shuffle"), "Shuffle"),
];

function emptyLibrary() {
  const s = Lib.scan;
  return h("div", { class: "lib-empty" },
    h("img", { src: "assets/mark.svg", alt: "", width: "110", height: "96" }),
    h("h2", {}, s ? "Reading your music…" : "Your library is empty"),
    h("p", {}, s ? `${s.done} of ${s.total} files` : "Songs you download land here automatically. You can also point Atmosphere at folders you already have."),
    s ? progressBar(s.total ? s.done / s.total : null) : h("div", { class: "row" }, h("button", { class: "btn primary", onclick: addFolder }, icon("folder-plus"), "Add a music folder"), h("button", { class: "btn", onclick: () => go("get") }, icon("cloud-download"), "Get music")));
}

function filtered(list) {
  const q = lnorm(Lib.filter);
  if (!q) return list;
  return list.filter((t) => `${t.title} ${t.artist} ${t.album} ${t.genre}`.toLowerCase().includes(q));
}

function sortedSongs() {
  const { key, dir } = Lib.sort;
  const val = (t) => (key === "plays" ? -(Lib.plays[t.id] || 0) : key === "added" ? -(t.added || 0) : key === "duration" ? t.durationMs : lnorm(key === "artist" ? `${t.albumArtist || t.artist}|${t.album}|${String(t.discNo || 1).padStart(2, "0")}${String(t.trackNo || 0).padStart(3, "0")}` : t[key]));
  return [...filtered(Lib.songs)].sort((a, b) => (val(a) > val(b) ? dir : val(a) < val(b) ? -dir : 0));
}

function songsView(root) {
  const list = sortedSongs();
  Lib.currentList = list;
  const sortSel = h("select", { "aria-label": "Sort by", onchange: (e) => { Lib.sort = { key: e.target.value, dir: 1 }; renderLib(); } },
    ...[["artist", "Artist"], ["title", "Title"], ["album", "Album"], ["added", "Recently added"], ["plays", "Most played"], ["duration", "Length"]].map(([v, l]) => h("option", { value: v, selected: Lib.sort.key === v }, l)));
  root.append(libHead("Songs", `${plural(Lib.songs.length, "song")}${Lib.scan ? ` · reading ${Lib.scan.done} of ${Lib.scan.total}…` : ""}`, [sortSel, ...playBtns(list)], { search: true, onSearch: () => renderLib(true) }));
  if (!Lib.songs.length) return root.append(emptyLibrary());
  const head = h("div", { class: "lt-head with-cover" }, h("span", {}, "#"), h("span"), h("span", {}, "Title"), h("span", {}, "Album"), h("span"), h("span", { class: "r" }, icon("clock")), h("span"));
  const box = h("div", { class: "vlist" });
  root.append(head, box);
  if (!list.length) return box.append(h("p", { class: "muted pad" }, "No songs match your search."));
  const ctx = { list, cover: true };
  const rowH = 52;
  box.addEventListener("click", (e) => { if (e.target === box || e.target === spacer) { Lib.sel.clear(); markSelection(); } });
  const spacer = h("div", { class: "vspacer", style: `height:${list.length * rowH}px` });
  box.append(spacer);
  const scroller = $("#libScroll");
  const draw = () => {
    const top = scroller.scrollTop - spacer.offsetTop;
    const start = Math.max(0, Math.floor(top / rowH) - 10);
    const end = Math.min(list.length, Math.ceil((top + scroller.clientHeight) / rowH) + 10);
    const rows = [];
    for (let i = start; i < end; i++) {
      const r = trackRow(list[i], i, ctx);
      r.style.transform = `translateY(${i * rowH}px)`;
      rows.push(r);
    }
    spacer.replaceChildren(...rows);
  };
  Lib.drawSongs = draw;
  requestAnimationFrame(draw);
}

function featuredCard(f) {
  const open = () => (f.album ? openLib("album", f.album.key) : playList([f.track]));
  const onLine = f.on.length && !(f.on.length === 1 && lnorm(f.on[0]) === lnorm(f.title)) ? `on ${f.on.slice(0, 2).join(", ")}${f.on.length > 2 ? ` +${f.on.length - 2}` : ""}` : "";
  return h("div", { class: "card-tile", role: "button", tabindex: "0", "data-album": f.album?.key || null, onclick: open, onkeydown: (e) => { if (e.key === "Enter") open(); } },
    h("div", { class: "tile-art" }, cover(f.art, "tile-img", f.title), h("button", { class: "tile-play", "aria-label": `Play ${f.title}`, onclick: (e) => { e.stopPropagation(); playList(f.album ? f.album.tracks : [f.track]); } }, icon("player-play-filled"))),
    h("div", { class: "tile-title" }, f.title),
    h("div", { class: "tile-sub" }, h("span", { class: "tile-kind" }, f.kind), ` · ${f.by}`),
    onLine ? h("div", { class: "tile-sub on" }, onLine) : null);
}

function albumCard(a) {
  return h("div", { class: "card-tile", role: "button", tabindex: "0", "data-album": a.key, onclick: () => openLib("album", a.key), onkeydown: (e) => { if (e.key === "Enter") openLib("album", a.key); } },
    h("div", { class: "tile-art" }, cover(a.art, "tile-img", a.title), h("button", { class: "tile-play", "aria-label": `Play ${a.title}`, onclick: (e) => { e.stopPropagation(); playList(a.tracks); } }, icon("player-play-filled"))),
    h("div", { class: "tile-title" }, a.title),
    h("div", { class: "tile-sub" }, [a.artist, a.year].filter(Boolean).join(" · ")));
}

function albumsView(root) {
  const q = lnorm(Lib.filter);
  const list = [...Lib.albums.values()].filter((a) => !q || `${a.title} ${a.artist}`.toLowerCase().includes(q)).sort((x, y) => lnorm(x.artist).localeCompare(lnorm(y.artist)) || (y.year || 0) - (x.year || 0));
  root.append(libHead("Albums", plural(Lib.albums.size, "album"), [], { search: true, onSearch: () => renderLib(true) }));
  if (!Lib.songs.length) return root.append(emptyLibrary());
  root.append(h("div", { class: "grid" }, ...list.map(albumCard)));
}

function albumView(root, key) {
  const a = Lib.albums.get(key);
  if (!a) return root.append(libHead("Album not found", "It may have been renamed or removed."));
  const discs = new Set(a.tracks.map((t) => t.discNo || 1)).size > 1;
  const multiArtist = a.tracks.some((t) => lnorm(primaryArtist(t.artist)) !== lnorm(a.artist));
  root.append(
    h("header", { class: "hero" },
      h("div", { class: "hero-bg", style: a.art ? `background-image:url("${artUrl(a.art)}")` : "" }),
      cover(a.art, "hero-cover", a.title),
      h("div", { class: "hero-meta" },
        h("div", { class: "kind-line" }, h("span", { class: "kind-btn", style: "cursor:default" }, albumKind(a)), a.year ? h("span", {}, String(a.year)) : null, a.tracks[0]?.genre ? h("span", { class: "sep" }, "·") : null, a.tracks[0]?.genre || null),
        h("h1", { class: "hero-title" }, a.title),
        h("button", { class: "link hero-artist", onclick: () => openLib("artist", lnorm(a.artist)) }, a.artist),
        h("p", { class: "view-sub" }, `${plural(a.tracks.length, "song")} · ${fmtLong(a.duration)}`),
        h("div", { class: "row" }, ...playBtns(a.tracks), h("button", { class: "btn ghost", onclick: (e) => menuAt(e.clientX, e.clientY, playlistItems(a.tracks)()) }, icon("playlist-add"), "Add to playlist"), h("button", { class: "btn ghost", onclick: () => editAlbum(a) }, icon("pencil"), "Edit"))))
  );
  if (Lib.back.length) root.firstChild.prepend(h("button", { class: "icon-btn back on-hero", "aria-label": "Back", onclick: libBack }, icon("arrow-left")));
  const ctx = { hideAlbum: true, hideArtist: !multiArtist, showArtistFor: (t) => lnorm(primaryArtist(t.artist)) !== lnorm(a.artist), numbers: (t, i) => t.trackNo || i + 1 };
  if (!discs) return root.append(h("div", { class: "pad-x" }, trackList(a.tracks, ctx)));
  const body = h("div", { class: "pad-x" });
  for (const d of [...new Set(a.tracks.map((t) => t.discNo || 1))]) {
    body.append(h("h3", { class: "disc" }, icon("disc"), `Disc ${d}`));
    body.append(trackList(a.tracks.filter((t) => (t.discNo || 1) === d), { ...ctx, list: a.tracks, numbers: (t) => t.trackNo || a.tracks.indexOf(t) + 1 }));
  }
  root.append(body);
}

function artistCard(ar) {
  return h("div", { class: "card-tile artist", role: "button", tabindex: "0", "data-artist": ar.key, onclick: () => openLib("artist", ar.key), onkeydown: (e) => { if (e.key === "Enter") openLib("artist", ar.key); } },
    h("div", { class: "tile-art round" }, cover(artistImage(ar), "tile-img", ar.name)),
    h("div", { class: "tile-title" }, ar.name),
    h("div", { class: "tile-sub" }, ar.albums.length ? plural(ar.albums.length, "release") : plural(ar.tracks.length, "song")));
}

function artistsView(root) {
  const q = lnorm(Lib.filter);
  const list = [...Lib.artists.values()].filter((a) => !q || a.key.includes(q)).sort((x, y) => x.key.localeCompare(y.key));
  root.append(libHead("Artists", plural(Lib.artists.size, "artist"), [], { search: true, onSearch: () => renderLib(true) }));
  if (!Lib.songs.length) return root.append(emptyLibrary());
  root.append(h("div", { class: "grid" }, ...list.map(artistCard)));
}

function artistView(root, key) {
  const ar = Lib.artists.get(key);
  if (!ar) return root.append(libHead("Artist not found", "Their songs may have been retagged or removed."));
  const img = artistImage(ar);
  const popular = [...ar.tracks].sort((a, b) => (Lib.plays[b.id] || 0) - (Lib.plays[a.id] || 0) || (b.added || 0) - (a.added || 0)).slice(0, 5);
  const plays = ar.tracks.reduce((n, t) => n + (Lib.plays[t.id] || 0), 0);
  const full = ar.albums.filter((a) => albumKind(a) === "Album");
  const small = ar.albums.filter((a) => albumKind(a) !== "Album");
  root.append(
    h("header", { class: "hero artist-hero" },
      h("div", { class: "hero-bg", style: img ? `background-image:url("${artUrl(img)}")` : "" }),
      cover(img, "avatar", ar.name),
      h("div", { class: "hero-meta" },
        h("div", { class: "kind-line" }, h("span", { class: "kind-btn", style: "cursor:default" }, icon("user"), "Artist")),
        ar.meta?.logo ? h("h1", { class: "hero-title big has-logo" }, h("img", { class: `artist-logo ${ar.meta.logoDark ? "dark" : ""}`, src: artUrl(ar.meta.logo), alt: ar.name })) : h("h1", { class: "hero-title big" }, ar.name),
        h("p", { class: "view-sub" }, [plural(ar.tracks.length, "song"), ar.albums.length && plural(ar.albums.length, "release"), plays && plural(plays, "play")].filter(Boolean).join(" · ")),
        h("div", { class: "row" }, ...playBtns(ar.tracks), h("button", { class: "btn ghost", onclick: () => editArtist(ar) }, icon("pencil"), "Edit profile"))))
  );
  if (Lib.back.length) root.firstChild.prepend(h("button", { class: "icon-btn back on-hero", "aria-label": "Back", onclick: libBack }, icon("arrow-left")));
  const body = h("div", { class: "pad-x sections" });
  body.append(h("section", {}, h("h2", { class: "sec" }, "Popular"), trackList(popular, { cover: true, list: popular })));
  if (full.length) body.append(h("section", {}, h("h2", { class: "sec" }, "Albums"), h("div", { class: "grid" }, ...full.map(albumCard))));
  if (small.length) body.append(h("section", {}, h("h2", { class: "sec" }, "Singles & EPs"), h("div", { class: "grid" }, ...small.map(albumCard))));
  if (ar.featured.length) body.append(h("section", {}, h("h2", { class: "sec" }, "Featured on"), h("div", { class: "grid" }, ...ar.featured.map(featuredCard))));
  body.append(h("section", { class: "about" }, h("h2", { class: "sec" }, "About"),
    ar.meta?.bio ? h("p", { class: "bio" }, ar.meta.bio) : h("button", { class: "link muted", onclick: () => editArtist(ar) }, "Add a bio and photo for this artist"),
    ar.meta?.bio && ar.meta?.bioSource ? h("button", { class: "link muted small source", onclick: () => invoke("open_link", { url: ar.meta.bioSource }).catch(() => {}) }, "From Wikipedia, under CC BY-SA") : null));
  if (!ar.meta?.checked) queueArtistInfo([ar.name], true);
  root.append(body);
}

function mosaic(tracks) {
  const arts = [...new Set(tracks.map((t) => t.art).filter(Boolean))].slice(0, 4);
  if (arts.length < 4) return arts.length ? cover(arts[0], "hero-cover") : h("div", { class: "hero-cover ph" }, icon("playlist"));
  return h("div", { class: "hero-cover mosaic" }, ...arts.map((k) => cover(k, "")));
}

function playlistView(root, name) {
  const p = (Lib.data?.playlists || []).find((x) => x.name === name);
  if (!p) return root.append(libHead("Playlist not found", "Its folder may have been moved or deleted."));
  if (p.missing) {
    root.append(
      libHead(p.name, "Linked folder"),
      h("div", { class: "lib-empty missing-folder" },
        icon("alert-circle"),
        h("h2", {}, "This folder can't be found"),
        h("p", {}, "It may have been moved or renamed, or it's on a drive that isn't connected. Atmosphere checks again every few seconds and brings it back as soon as it's there."),
        h("code", { class: "path" }, p.path),
        h("div", { class: "row center" },
          h("button", { class: "btn primary", onclick: () => relocateFolder(p.name) }, icon("folder-open"), "Locate folder…"),
          h("button", { class: "btn ghost", onclick: () => unlinkFolder(p.name) }, "Remove from Atmosphere")))
    );
    return;
  }
  const tracks = p.order.map((id) => Lib.byId.get(id)).filter(Boolean);
  const dur = tracks.reduce((n, t) => n + (t.durationMs || 0), 0);
  const title = h("h1", { class: "hero-title editable", contenteditable: "plaintext-only", spellcheck: "false", "aria-label": "Playlist name", onkeydown: (e) => { if (e.key === "Enter") { e.preventDefault(); e.target.blur(); } } }, p.name);
  title.addEventListener("blur", async () => {
    const v = title.textContent.trim();
    if (!v || v === p.name) return (title.textContent = p.name);
    try {
      const n = await invoke("playlist_rename", { from: p.name, to: v });
      Lib.route = { name: "playlist", id: n };
    } catch (e) {
      toast(errText(e), "err");
      title.textContent = p.name;
    }
  });
  root.append(
    h("header", { class: "hero" },
      h("div", { class: "hero-bg", style: tracks[0]?.art ? `background-image:url("${artUrl(tracks[0].art)}")` : "" }),
      p.cover ? cover(p.cover, "hero-cover", p.name) : mosaic(tracks),
      h("div", { class: "hero-meta" },
        h("div", { class: "kind-line" }, h("span", { class: "kind-btn", style: "cursor:default" }, icon(p.linked ? "link" : "folder"), p.linked ? "Your folder" : "Playlist folder")),
        title,
        h("p", { class: "view-sub" }, `${plural(tracks.length, "song")}${dur ? " · " + fmtLong(dur) : ""}`),
        h("button", { class: "link path", title: "Open this folder", onclick: () => invoke("reveal", { path: p.path }) }, p.path),
        h("div", { class: "row" }, ...playBtns(tracks),
          h("button", { class: "btn ghost", onclick: async () => { try { const n = await invoke("playlist_add_files", { name: p.name }); if (n) toast(`Added ${plural(n, "song")}`, "ok"); } catch (e) { toast(errText(e), "err"); } } }, icon("plus"), "Add files…"),
          h("button", { class: "btn ghost", onclick: () => editPlaylist(p) }, icon("pencil"), "Edit"),
          p.linked
            ? h("button", { class: "btn ghost", onclick: () => unlinkFolder(p.name) }, icon("x"), "Remove from Atmosphere")
            : h("button", { class: "btn ghost danger", onclick: async () => { if (await confirmBox(`Delete “${p.name}”?`, "The playlist folder goes to your Recycle Bin or Trash. Songs in your library aren't touched.", "Delete")) { await invoke("playlist_delete", { name: p.name }).catch((e) => toast(errText(e), "err")); openLib("songs", null, false); } } }, icon("trash"), "Delete"))))
  );
  if (Lib.back.length) root.firstChild.prepend(h("button", { class: "icon-btn back on-hero", "aria-label": "Back", onclick: libBack }, icon("arrow-left")));
  if (!tracks.length) return root.append(h("div", { class: "lib-empty slim" }, h("p", {}, "Drag songs onto this playlist in the sidebar, use “Add to playlist” from any song's menu, or add files from your computer.")));
  root.append(h("div", { class: "pad-x" }, h("p", { class: "muted small hint-line" }, p.linked ? "This is your own folder; songs in subfolders are included. Drag rows to reorder, saved in a playlist.m3u8 inside it." : "Drag rows to reorder. The order is saved in the folder's playlist.m3u8, so other players see it too."), trackList(tracks, { cover: true, playlist: p.name })));
}

function greeting() {
  const hr = new Date().getHours();
  return hr < 5 ? "Up late" : hr < 12 ? "Good morning" : hr < 18 ? "Good afternoon" : "Good evening";
}

function trackCard(t) {
  return h("div", { class: "card-tile", role: "button", tabindex: "0", "data-id": t.id, onclick: () => playList([t]), onkeydown: (e) => { if (e.key === "Enter") playList([t]); } },
    h("div", { class: "tile-art" }, cover(t.art, "tile-img", t.title), h("button", { class: "tile-play", "aria-label": `Play ${t.title}`, onclick: (e) => { e.stopPropagation(); playList([t]); } }, icon("player-play-filled"))),
    h("div", { class: "tile-title" }, t.title),
    h("div", { class: "tile-sub" }, h("span", { class: "tile-kind" }, "Song"), ` · ${primaryArtist(t.artist)}`));
}

function playlistCard(p) {
  const tracks = p.order.map((id) => Lib.byId.get(id)).filter(Boolean);
  const arts = [...new Set(tracks.map((t) => t.art).filter(Boolean))].slice(0, 4);
  const art = p.cover ? cover(p.cover, "tile-img", p.name) : arts.length >= 4 ? h("div", { class: "tile-img tile-mosaic" }, ...arts.map((k) => cover(k, ""))) : arts.length ? cover(arts[0], "tile-img", p.name) : h("div", { class: "tile-img ph" }, icon(p.linked ? "folder" : "playlist"));
  return h("div", { class: "card-tile", role: "button", tabindex: "0", "data-playlist": p.name, onclick: () => openLib("playlist", p.name), onkeydown: (e) => { if (e.key === "Enter") openLib("playlist", p.name); } },
    h("div", { class: "tile-art" }, art, tracks.length ? h("button", { class: "tile-play", "aria-label": `Play ${p.name}`, onclick: (e) => { e.stopPropagation(); playList(tracks); } }, icon("player-play-filled")) : null),
    h("div", { class: "tile-title" }, p.name),
    h("div", { class: "tile-sub" }, h("span", { class: "tile-kind" }, "Playlist"), ` · ${plural(tracks.length, "song")}`));
}

function contextsFrom(tracks, limit) {
  const out = [];
  const seen = new Set();
  for (const t of tracks) {
    const a = t.album ? Lib.albums.get(`${lnorm(t.albumArtist || primaryArtist(t.artist))}|${lnorm(t.album)}`) : null;
    const key = a ? `a:${a.key}` : `t:${t.id}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(a && a.tracks.length > 1 ? albumCard(a) : trackCard(t));
    if (out.length >= limit) break;
  }
  return out;
}

function shelf(title, cards, more) {
  if (!cards.length) return null;
  return h("section", { class: "shelf-sec" },
    h("div", { class: "sec-row" }, h("h2", { class: "sec" }, title), more ? h("button", { class: "link muted small", onclick: more }, "Show all") : null),
    h("div", { class: "grid shelf" }, ...cards));
}

function homeView(root) {
  const songs = Lib.songs;
  root.append(
    h("header", { class: "home-head" },
      h("div", {},
        h("h1", {}, greeting()),
        h("p", { class: "view-sub" }, songs.length ? [plural(songs.length, "song"), plural(Lib.albums.size, "album"), plural(Lib.artists.size, "artist")].join(" · ") : "Let's get some music in here.")),
      songs.length
        ? h("div", { class: "row home-quick" },
            h("button", { class: "btn primary", onclick: () => playList(songs, Math.floor(Math.random() * songs.length), true) }, icon("arrows-shuffle"), "Shuffle everything"),
            Lib.liked.size ? h("button", { class: "btn", onclick: () => openLib("liked") }, icon("heart"), "Liked songs") : null,
            h("button", { class: "btn ghost", onclick: () => openLib("stats") }, icon("chart-bar"), "Your stats"))
        : null)
  );
  if (!songs.length) return root.append(emptyLibrary());
  const last = Lib.data?.lastPlayed || {};
  const recent = songs.filter((t) => last[t.id]).sort((a, b) => last[b.id] - last[a.id]);
  const added = [...songs].sort((a, b) => (b.added || 0) - (a.added || 0));
  const top = songs.filter((t) => Lib.plays[t.id]).sort((a, b) => (Lib.plays[b.id] || 0) - (Lib.plays[a.id] || 0)).slice(0, 5);
  const lists = (Lib.data?.playlists || []).filter((p) => !p.missing && p.order.length);
  const body = h("div", { class: "pad-x sections home" },
    shelf("Jump back in", contextsFrom(recent, 12)),
    shelf("Recently added", contextsFrom(added, 12), () => { Lib.sort = { key: "added", dir: 1 }; openLib("songs"); }),
    top.length ? h("section", { class: "shelf-sec" }, h("div", { class: "sec-row" }, h("h2", { class: "sec" }, "On repeat"), h("button", { class: "link muted small", onclick: () => { Lib.sort = { key: "plays", dir: 1 }; openLib("songs"); } }, "Show all")), trackList(top, { cover: true, list: top })) : null,
    shelf("Your playlists", lists.slice(0, 12).map(playlistCard)));
  if (!recent.length && !top.length) body.prepend(h("p", { class: "muted home-hint" }, "Play a few songs and this page fills up with your favourites."));
  root.append(body);
}

function likedView(root) {
  const list = filtered(Lib.songs.filter((t) => Lib.liked.has(t.id)));
  root.append(libHead("Liked songs", plural(list.length, "song"), playBtns(list), { search: true, onSearch: () => renderLib(true) }));
  if (!list.length) return root.append(h("div", { class: "lib-empty slim" }, icon("heart"), h("p", {}, "Tap the heart on any song to keep it here.")));
  root.append(h("div", { class: "pad-x" }, trackList(list, { cover: true })));
}

function renderLib(soft = false) {
  const root = $("#libView");
  const scroller = $("#libScroll");
  const y = soft ? scroller.scrollTop : 0;
  const focusSearch = soft && document.activeElement?.classList.contains("lib-search");
  const caret = focusSearch ? document.activeElement.selectionStart : 0;
  root.replaceChildren();
  Lib.drawSongs = null;
  const r = Lib.route;
  ({ songs: songsView, albums: albumsView, album: (x) => albumView(x, r.id), artists: artistsView, artist: (x) => artistView(x, r.id), playlist: (x) => playlistView(x, r.id), liked: likedView, stats: statsView, home: homeView }[r.name] || homeView)(root);
  scroller.scrollTop = y;
  if (focusSearch) {
    const s = $(".lib-search");
    s?.focus();
    s?.setSelectionRange(caret, caret);
  }
  markLibNav();
}

/* library + storage settings */
async function addFolder() {
  try {
    const p = await invoke("library_add_folder");
    if (p) {
      state.settings = await invoke("get_settings");
      renderLibrarySettings();
      toast("Reading the new folder…", "ok");
    }
  } catch (e) {
    toast(errText(e), "err");
  }
}

function renderLibrarySettings() {
  const s = state.settings;
  if (!s || !$("#libFolders")) return;
  $("#libFolders").replaceChildren(
    ...s.libraryFolders.map((f) => h("div", { class: "setting" }, h("div", {}, h("strong", { class: "mono-path" }, f), h("span", {}, f === s.outputDir ? "Your download folder" : "Music folder")),
      h("button", { class: "btn ghost danger", disabled: s.libraryFolders.length === 1, onclick: async () => { await invoke("library_remove_folder", { path: f }); state.settings = await invoke("get_settings"); renderLibrarySettings(); } }, "Remove"))),
    h("div", { class: "setting" }, h("div", {}, h("strong", {}, "Add a folder"), h("span", {}, "Atmosphere reads tags and cover art. It never moves or changes your files unless you edit them.")),
      h("div", { class: "row" }, h("button", { class: "btn", onclick: () => invoke("library_scan").then(() => toast("Rescanning your library…", "ok")) }, icon("refresh"), "Rescan"), h("button", { class: "btn primary", onclick: addFolder }, icon("folder-plus"), "Add folder…")))
  );
  $("#playlistsDir").textContent = s.playlistsDir;
  const hidden = Lib.data?.hidden || 0;
  $("#hiddenSongs").replaceChildren(
    h("div", {}, h("strong", {}, "Hidden songs"), h("span", {}, hidden ? `${songs(hidden)} removed from the library but still on your drive.` : "Songs you remove from the library (without deleting) appear here.")),
    h("button", { class: "btn", disabled: !hidden, onclick: async () => { const n = await invoke("tracks_unhide_all"); toast(`Restoring ${songs(n)}…`, "ok"); } }, "Restore all"));
}

async function backupNow() {
  const b = $("#backupBtn");
  const d = new Date();
  const name = `Atmosphere backup ${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}.zip`;
  b.classList.add("loading");
  b.disabled = true;
  try {
    const r = await invoke("backup_save", { name });
    if (r) toast(`Backed up ${r[1]} items to ${r[0].split(/[\\/]/).pop()}`, "ok");
  } catch (e) {
    toast(errText(e), "err");
  } finally {
    b.disabled = false;
    b.classList.remove("loading");
  }
}

async function restoreNow() {
  const path = await invoke("backup_pick").catch(() => null);
  if (!path) return;
  const body = h("p", {}, "Your current settings, stats, liked songs and artist photos will be replaced by the ones in ", h("strong", {}, path.split(/[\\/]/).pop()), ". Your music files stay as they are. Atmosphere restarts afterwards.");
  const ok = await modal("Restore this backup?", body, [{ label: "Cancel", value: false }, { label: "Restore and restart", value: true, primary: true }]);
  if (!ok) return;
  try {
    await invoke("backup_restore", { path });
  } catch (e) {
    toast(errText(e), "err");
  }
}

function bindBackup() {
  $("#backupBtn")?.addEventListener("click", backupNow);
  $("#restoreBtn")?.addEventListener("click", restoreNow);
}

async function renderStorage() {
  const box = $("#storageList");
  if (!box) return;
  const items = await invoke("storage_report").catch(() => []);
  const fmt = (b) => fmtBytes(b);
  const cleanable = items.filter((i) => i.cleanable && i.bytes > 0);
  box.replaceChildren(
    ...items.map((i) => h("div", { class: "setting" }, h("div", {}, h("strong", {}, i.label), h("span", {}, i.detail)),
      h("div", { class: "row" }, h("span", { class: "size" }, i.count ? `${fmt(i.bytes)} · ${plural(i.count, "file")}` : "Empty"),
        i.cleanable ? h("button", { class: "btn", disabled: !i.bytes, onclick: () => cleanStorage([i.id]) }, "Clean") : null))),
    h("div", { class: "setting" }, h("div", {}, h("strong", {}, "Clean everything that's safe to remove"), h("span", {}, "Never touches your music, playlists or settings.")),
      h("button", { class: "btn primary", disabled: !cleanable.length, onclick: () => cleanStorage(cleanable.map((i) => i.id)) }, "Clean up"))
  );
}

async function cleanStorage(ids) {
  const r = await invoke("storage_clean", { ids }).catch((e) => (toast(errText(e), "err"), null));
  if (r) toast(r.files ? `Removed ${plural(r.files, "file")} (${fmtBytes(r.bytes)})` : "Nothing needed cleaning", "ok");
  renderStorage();
}

/* boot */
function bindLibrary() {
  $$("[data-lib]").forEach((b) => b.addEventListener("click", () => { Lib.back = []; openLib(b.dataset.lib, null, false); }));
  $("#newPlaylistBtn").addEventListener("click", playlistAddMenu);
  $("#libScroll").addEventListener("scroll", () => Lib.drawSongs && requestAnimationFrame(Lib.drawSongs), { passive: true });
  window.addEventListener("resize", () => Lib.drawSongs && Lib.drawSongs());
  window.addEventListener("player", (e) => { if (e.detail.type === "track") markPlaying(); });
  window.addEventListener("played", (e) => { Lib.plays[e.detail] = (Lib.plays[e.detail] || 0) + 1; });
  listen("atmosphere://library", (e) => {
    const p = e.payload;
    if (p.type === "progress") {
      Lib.scan = { done: p.done, total: p.total };
      if (state.view === "library" && !Lib.songs.length) renderLib(true);
    } else {
      Lib.scan = null;
      if (p.type === "error") toast(p.message, "err");
      clearTimeout(Lib.reload);
      Lib.reload = setTimeout(loadLibrary, 150);
    }
  });
  loadLibrary();
}

/* app-wide context menus (replaces the browser's own) */
const MOD = isMac ? "⌘" : "Ctrl+";

function editMenu(e, el) {
  const isInput = el.tagName === "INPUT" || el.tagName === "TEXTAREA";
  const start = isInput ? el.selectionStart : null;
  const end = isInput ? el.selectionEnd : null;
  const sel = window.getSelection();
  const range = !isInput && sel.rangeCount ? sel.getRangeAt(0).cloneRange() : null;
  const hasSel = isInput ? start !== end : !sel.isCollapsed;
  const readOnly = el.readOnly || el.disabled;
  const restore = () => {
    el.focus();
    if (isInput) el.setSelectionRange(start, end);
    else if (range) { sel.removeAllRanges(); sel.addRange(range); }
  };
  const selected = () => (isInput ? el.value.slice(start, end) : range?.toString() || "");
  menuAt(e.clientX, e.clientY, [
    { label: "Cut", hint: `${MOD}X`, disabled: !hasSel || readOnly, run: async () => { await invoke("clipboard_write", { text: selected() }); restore(); document.execCommand("delete"); el.dispatchEvent(new Event("input", { bubbles: true })); } },
    { label: "Copy", hint: `${MOD}C`, disabled: !hasSel, run: () => invoke("clipboard_write", { text: selected() }) },
    { label: "Paste", hint: `${MOD}V`, disabled: readOnly, run: async () => { const text = await invoke("clipboard_text").catch(() => ""); restore(); if (text) document.execCommand("insertText", false, text); } },
    "-",
    { label: "Select all", hint: `${MOD}A`, run: () => { el.focus(); if (isInput) el.select(); else document.execCommand("selectAll"); } },
  ]);
}

function albumMenu(e, a) {
  menuAt(e.clientX, e.clientY, [
    { label: "Play", run: () => playList(a.tracks, 0, false) },
    { label: "Shuffle", run: () => playList(a.tracks, Math.floor(Math.random() * a.tracks.length), true) },
    { label: "Play next", run: () => Player.playNext(a.tracks) },
    { label: "Add to queue", run: () => Player.addToQueue(a.tracks) },
    "-",
    { label: "Add to playlist", sub: playlistItems(a.tracks) },
    { label: "Go to artist", run: () => openLib("artist", lnorm(a.artist)) },
    { label: `Edit ${kindWord(a)}…`, run: () => editAlbum(a) },
    "-",
    { label: "Remove album from library", run: () => hideTracks(a.tracks) },
    { label: "Delete album from device…", danger: true, run: () => deleteTracks(a.tracks) },
  ]);
}

function artistMenu(e, ar) {
  menuAt(e.clientX, e.clientY, [
    { label: "Play", run: () => playList(ar.tracks, 0, false) },
    { label: "Shuffle", run: () => playList(ar.tracks, Math.floor(Math.random() * ar.tracks.length), true) },
    { label: "Add to queue", run: () => Player.addToQueue(ar.tracks) },
    "-",
    { label: "Open profile", run: () => openLib("artist", ar.key) },
    { label: "Edit profile…", run: () => editArtist(ar) },
  ]);
}

function playlistMenu(e, name) {
  const p = (Lib.data?.playlists || []).find((x) => x.name === name);
  if (!p) return;
  const tracks = p.order.map((id) => Lib.byId.get(id)).filter(Boolean);
  if (p.missing) {
    return menuAt(e.clientX, e.clientY, [
      { label: "Locate folder…", run: () => relocateFolder(name) },
      "-",
      { label: "Remove from Atmosphere…", run: () => unlinkFolder(name) },
    ]);
  }
  if (p.linked) {
    return menuAt(e.clientX, e.clientY, [
      { label: "Play", disabled: !tracks.length, run: () => playList(tracks, 0, false) },
      { label: "Shuffle", disabled: !tracks.length, run: () => playList(tracks, Math.floor(Math.random() * tracks.length), true) },
      "-",
      { label: "Add files…", run: () => invoke("playlist_add_files", { name }).catch((err) => toast(errText(err), "err")) },
      { label: "Edit…", run: () => editPlaylist(p) },
      { label: "Show in folder", run: () => invoke("reveal", { path: p.path }) },
      { label: "Locate a different folder…", run: () => relocateFolder(name) },
      "-",
      { label: "Remove from Atmosphere…", run: () => unlinkFolder(name) },
    ]);
  }
  menuAt(e.clientX, e.clientY, [
    { label: "Play", disabled: !tracks.length, run: () => playList(tracks, 0, false) },
    { label: "Shuffle", disabled: !tracks.length, run: () => playList(tracks, Math.floor(Math.random() * tracks.length), true) },
    "-",
    { label: "Add files…", run: () => invoke("playlist_add_files", { name }).catch((err) => toast(errText(err), "err")) },
    { label: "Edit…", run: () => editPlaylist(p) },
    { label: "Show in folder", run: () => invoke("reveal", { path: p.path }) },
    "-",
    { label: "Delete playlist…", danger: true, run: async () => { if (await confirmBox(`Delete “${name}”?`, "The playlist folder goes to your Recycle Bin or Trash. Songs in your library aren't touched.", "Delete")) invoke("playlist_delete", { name }).catch((err) => toast(errText(err), "err")); } },
  ]);
}

document.addEventListener("contextmenu", (e) => {
  if (e.defaultPrevented) return;
  e.preventDefault();
  const t = e.target;
  const field = t.closest("input:not([type=checkbox]):not([type=range]):not([type=radio]), textarea, [contenteditable]");
  if (field) return editMenu(e, field);
  const selText = window.getSelection()?.toString().trim();
  if (selText) return menuAt(e.clientX, e.clientY, [{ label: "Copy", hint: `${MOD}C`, run: () => invoke("clipboard_write", { text: selText }) }]);
  const album = t.closest("[data-album]");
  if (album) return Lib.albums.has(album.dataset.album) && albumMenu(e, Lib.albums.get(album.dataset.album));
  const artist = t.closest("[data-artist]");
  if (artist) return Lib.artists.has(artist.dataset.artist) && artistMenu(e, Lib.artists.get(artist.dataset.artist));
  const pl = t.closest("[data-playlist]");
  if (pl) return playlistMenu(e, pl.dataset.playlist);
  const q = t.closest("[data-qi]");
  if (q) {
    const i = Number(q.dataset.qi);
    const tr = Player.queue[i];
    return tr && menuAt(e.clientX, e.clientY, [
      { label: "Play", run: () => Player.load(i) },
      { label: "Remove from queue", disabled: i === Player.index, run: () => Player.removeAt(i) },
      "-",
      { label: "Add to playlist", sub: playlistItems([tr]) },
      { label: "Show in folder", run: () => invoke("reveal", { path: tr.path }) },
    ]);
  }
  const line = t.closest(".lyr-line");
  if (line && Lyr.data) {
    const l = Lyr.data.lines[Number(line.dataset.i)];
    return menuAt(e.clientX, e.clientY, [
      { label: "Copy line", run: () => invoke("clipboard_write", { text: l.text }) },
      { label: "Copy all lyrics", run: () => invoke("clipboard_write", { text: Lyr.data.lines.map((x) => x.text).join("\n") }) },
      l.t != null ? { label: "Play from here", run: () => Player.seek((l.t - Lyr.offset) / 1000) } : null,
    ]);
  }
  const fetched = t.closest(".fetched-item");
  if (fetched) {
    const c = state.collections.find((x) => x.data.id === fetched.dataset.cid);
    return c && menuAt(e.clientX, e.clientY, [
      { label: "Open", run: () => { state.active = c; go("get"); renderFetched(); renderSheet(); } },
      c.data.url ? { label: "Open on site", run: () => invoke("open_link", { url: c.data.url }) } : null,
      c.data.url ? { label: "Copy link", run: () => invoke("clipboard_write", { text: c.data.url }) } : null,
      "-",
      { label: "Remove", run: () => removeCollection(c) },
    ]);
  }
  const hist = t.closest("[data-hist]");
  if (hist) {
    const r = state.history.find((x) => x.id === hist.dataset.hist);
    return r && menuAt(e.clientX, e.clientY, [
      r.path ? { label: "Show in folder", run: () => invoke("reveal", { path: r.path }) } : null,
      r.url ? { label: "Open source page", run: () => invoke("open_link", { url: r.url }) } : null,
      r.url ? { label: "Copy link", run: () => invoke("clipboard_write", { text: r.url }) } : null,
      "-",
      { label: "Remove from history", run: async () => { await invoke("history_remove", { ids: [r.id] }); state.history = state.history.filter((x) => x !== r); renderHistory(); } },
    ]);
  }
  const srow = t.closest(".tl-row[data-index]");
  if (srow && current()) {
    const c = current();
    const idx = Number(srow.dataset.index);
    const ent = entryOf(c, idx);
    return ent && menuAt(e.clientX, e.clientY, [
      { label: c.selected.has(idx) ? "Don't download" : "Include in download", disabled: !selectable(ent), run: () => { c.selected.has(idx) ? c.selected.delete(idx) : c.selected.add(idx); renderSheet(true); } },
      { label: "Get info", run: () => { c.inspected = idx; renderSheet(true); } },
      ent.url ? { label: "Copy link", run: () => invoke("clipboard_write", { text: ent.url }) } : null,
    ]);
  }
});

document.addEventListener("keydown", (e) => {
  const mod = e.ctrlKey || e.metaKey;
  const k = e.key.toLowerCase();
  if (e.key === "F5" || (mod && ["r", "p", "s", "u", "j", "g", "o", "n"].includes(k)) || (mod && e.shiftKey && ["r", "p"].includes(k))) {
    e.preventDefault();
    return;
  }
  const typing = e.target.closest?.("input, textarea, select, [contenteditable]");
  if (!typing && state.view === "library" && Lib.sel.size && (e.key === "Delete" || (isMac && e.key === "Backspace" && e.metaKey))) {
    e.preventDefault();
    return deleteTracks(selectedTracks());
  }
  if (!typing && state.view === "library" && mod && k === "a" && Lib.currentList.length) {
    e.preventDefault();
    Lib.sel = new Set(Lib.currentList.map((t) => t.id));
    return markSelection();
  }
  if (!typing && e.key === "Escape" && Lib.sel.size) {
    Lib.sel.clear();
    markSelection();
  }
  if (mod && k === "f") {
    e.preventDefault();
    const s = state.view === "library" ? $(".lib-search") : null;
    if (s) { s.focus(); s.select(); }
    else { go("get"); $("#commandInput").focus(); }
  }
}, true);
