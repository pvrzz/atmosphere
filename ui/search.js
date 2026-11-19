// atmosphere · search.js · Quick search palette (Ctrl+K) across songs, albums, artists and playlists, with link and online search shortcuts
const QS = { open: false, items: [], sel: 0, timer: 0 };

function qsScore(hay, q, words) {
  if (!hay) return 0;
  if (hay === q) return 100;
  if (hay.startsWith(q)) return 80;
  if (words.every((w) => hay.includes(w))) return hay.split(/[\s\-–(]+/).some((p) => p.startsWith(words[0])) ? 60 : 40;
  return 0;
}

function qsResults(raw) {
  const q = lnorm(raw);
  if (!q) return [];
  const words = q.split(/\s+/).filter(Boolean);
  const out = [];
  if (/^(https?:\/\/|www\.)\S+$/i.test(raw.trim())) out.push({ kind: "link", title: "Get this link", sub: raw.trim(), icon: "link", run: () => qsOnline(raw.trim(), null) });
  const songs = [];
  for (const t of Lib.songs) {
    const s = Math.max(qsScore(lnorm(t.title), q, words), qsScore(lnorm(`${t.title} ${t.artist}`), q, words) - 5, qsScore(lnorm(`${t.artist} ${t.title}`), q, words) - 8);
    if (s) songs.push([s + Math.min(15, (Lib.plays[t.id] || 0) / 2), t]);
  }
  songs.sort((a, b) => b[0] - a[0]);
  const albums = [...Lib.albums.values()].map((a) => [Math.max(qsScore(lnorm(a.title), q, words), qsScore(lnorm(`${a.title} ${a.artist}`), q, words) - 5), a]).filter((x) => x[0]).sort((a, b) => b[0] - a[0]);
  const artists = [...Lib.artists.values()].map((a) => [qsScore(a.key, q, words) + Math.min(10, a.tracks.length / 5), a]).filter((x) => x[0] > 10).sort((a, b) => b[0] - a[0]);
  const lists = (Lib.data?.playlists || []).map((p) => [qsScore(lnorm(p.name), q, words), p]).filter((x) => x[0]).sort((a, b) => b[0] - a[0]);
  const top = [songs[0], albums[0], artists[0], lists[0]].filter(Boolean).sort((a, b) => b[0] - a[0])[0];
  const add = (group, list, n, make) => list.slice(0, n).forEach(([, x], i) => out.push({ group: i === 0 ? group : null, ...make(x) }));
  const songItem = (t) => ({ kind: "song", title: t.title, sub: [t.artist, t.album].filter(Boolean).join(" · "), art: t.art, run: () => playList([t]), alt: () => Player.playNext([t]) });
  const albumItem = (a) => ({ kind: "album", title: a.title, sub: `${albumKind(a)} · ${a.artist}`, art: a.art, run: () => openLib("album", a.key), alt: () => playList(a.tracks) });
  const artistItem = (a) => ({ kind: "artist", title: a.name, sub: `Artist · ${plural(a.tracks.length, "song")}`, art: a.meta?.image || a.albums[0]?.art, round: true, run: () => openLib("artist", a.key), alt: () => playList(a.tracks, 0, true) });
  const listItem = (p) => ({ kind: "playlist", title: p.name, sub: `Playlist · ${plural(p.order.length, "song")}`, icon: p.linked ? "folder" : "playlist", run: () => openLib("playlist", p.name), alt: () => playList(p.order.map((id) => Lib.byId.get(id)).filter(Boolean)) });
  if (top && top[0] >= 60) {
    const x = top[1];
    out.push({ group: "Top result", ...(songs[0] === top ? songItem(x) : albums[0] === top ? albumItem(x) : artists[0] === top ? artistItem(x) : listItem(x)), top: true });
  }
  add("Songs", songs.filter((s) => !(top && s === top)), 6, songItem);
  add("Albums", albums.filter((s) => !(top && s === top)), 4, albumItem);
  add("Artists", artists.filter((s) => !(top && s === top)), 4, artistItem);
  add("Playlists", lists.filter((s) => !(top && s === top)), 4, listItem);
  if (!out.some((x) => x.kind === "link")) {
    out.push({ group: "Find online", kind: "online", title: `Search YouTube for “${raw.trim()}”`, icon: "brand-youtube", run: () => qsOnline(raw.trim(), "youtube") });
    out.push({ kind: "online", title: `Search SoundCloud for “${raw.trim()}”`, icon: "brand-soundcloud", run: () => qsOnline(raw.trim(), "soundcloud") });
  }
  return out;
}

function qsOnline(text, scope) {
  closeSearch();
  if (scope) {
    state.scope = scope;
    $$("#searchScope button").forEach((x) => x.setAttribute("aria-checked", String(x.dataset.scope === scope)));
  }
  go("get");
  $("#commandInput").value = text;
  $("#commandInput").dispatchEvent(new Event("input"));
  fetchText(text);
}

function qsRender() {
  const list = $("#qsList");
  if (!list) return;
  const raw = $("#qsInput").value;
  if (!raw.trim()) {
    list.replaceChildren(h("p", { class: "qs-hint" }, "Search your songs, albums, artists and playlists. Paste a link to get it."));
    QS.items = [];
    return;
  }
  QS.items = qsResults(raw);
  QS.sel = Math.min(QS.sel, Math.max(0, QS.items.length - 1));
  if (!QS.items.length) {
    list.replaceChildren(h("p", { class: "qs-hint" }, "Nothing matches."));
    return;
  }
  list.replaceChildren(
    ...QS.items.flatMap((it, i) => [
      it.group ? h("div", { class: "qs-group" }, it.group) : null,
      h("div", { class: `qs-item ${i === QS.sel ? "sel" : ""} ${it.top ? "top" : ""}`, role: "option", "aria-selected": String(i === QS.sel), "data-i": i, onmousemove: () => qsSelect(i), onclick: (e) => qsRun(i, e.shiftKey) },
        it.art ? cover(it.art, `qs-art ${it.round ? "round" : ""}`) : h("span", { class: "qs-art qs-ico" }, icon(it.icon || "music")),
        h("div", { class: "qs-text" }, h("span", { class: "qs-title" }, it.title), it.sub ? h("span", { class: "qs-sub" }, it.sub) : null),
        it.alt ? h("span", { class: "qs-keys" }, h("kbd", {}, "↵"), it.kind === "song" ? " play · " : " open · ", h("kbd", {}, "⇧↵"), it.kind === "song" ? " play next" : " play") : null),
    ]).filter(Boolean)
  );
}

function qsSelect(i) {
  if (i === QS.sel) return;
  QS.sel = i;
  $$(".qs-item").forEach((el) => {
    const on = Number(el.dataset.i) === i;
    el.classList.toggle("sel", on);
    el.setAttribute("aria-selected", String(on));
  });
}

function qsRun(i, alt) {
  const it = QS.items[i];
  if (!it) return;
  closeSearch();
  (alt && it.alt ? it.alt : it.run)();
}

function openSearch(prefill = "") {
  if (QS.open) return $("#qsInput").select();
  QS.open = true;
  QS.sel = 0;
  const input = h("input", { class: "qs-input", id: "qsInput", type: "search", placeholder: "Search your music, or paste a link", spellcheck: "false", autocomplete: "off", "aria-label": "Search", value: prefill });
  const wrap = h("div", { class: "qs-wrap", id: "qsWrap", onmousedown: (e) => { if (e.target === wrap) closeSearch(); } },
    h("div", { class: "qs", role: "dialog", "aria-modal": "true", "aria-label": "Search" },
      h("div", { class: "qs-bar" }, icon("search"), input, h("kbd", {}, "Esc")),
      h("div", { class: "qs-list", id: "qsList", role: "listbox" })));
  input.addEventListener("input", () => {
    clearTimeout(QS.timer);
    QS.sel = 0;
    QS.timer = setTimeout(qsRender, Lib.songs.length > 5000 ? 60 : 0);
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = QS.items.length;
      if (!n) return;
      qsSelect((QS.sel + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
      $(".qs-item.sel")?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      qsRun(QS.sel, e.shiftKey);
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeSearch();
    }
  });
  document.body.append(wrap);
  input.focus();
  qsRender();
}

function closeSearch() {
  QS.open = false;
  $("#qsWrap")?.remove();
}

function bindSearch() {
  $("#navSearch")?.addEventListener("click", () => openSearch());
}
