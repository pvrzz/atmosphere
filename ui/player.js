/* atmosphere · player.js · Audio engine: queue, shuffle/repeat, EQ presets, filters, reverb, 8D, pitch and tempo, media keys and resume. */
const MEDIA_BASE = /Windows/.test(navigator.userAgent) ? "http://atmo.localhost/" : "atmo://localhost/";
const mediaUrl = (kind, id) => `${MEDIA_BASE}${kind}/${encodeURIComponent(id)}`;
const artUrl = (key) => (key ? mediaUrl("a", key) : null);

const EQ_BANDS = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
const FX_DEFAULT = { preset: "flat", eq: EQ_BANDS.map(() => 0), tempo: 1, pitch: 0, vinyl: false, lowpass: 20000, highpass: 20, reverb: 0, wide8d: false };

const PRESETS = [
  { id: "flat", name: "Flat", desc: "No changes", group: "Tone", fx: {} },
  { id: "bass", name: "Bass boost", desc: "Fuller low end", group: "Tone", fx: { eq: [7, 6, 4, 2, 0, 0, 0, 0, 0, 0] } },
  { id: "treble", name: "Treble boost", desc: "Crisper highs", group: "Tone", fx: { eq: [0, 0, 0, 0, 0, 1, 2, 4, 5, 6] } },
  { id: "vocal", name: "Vocal", desc: "Voices up front", group: "Tone", fx: { eq: [-3, -2, -1, 1, 3, 4, 3, 1, 0, -1] } },
  { id: "loud", name: "Loudness", desc: "Punchy at low volume", group: "Tone", fx: { eq: [5, 4, 2, 0, -1, 0, 1, 3, 4, 4] } },
  { id: "rock", name: "Rock", desc: "Big edges, scooped mids", group: "Tone", fx: { eq: [4, 3, 2, 0, -1, -1, 1, 2, 3, 4] } },
  { id: "hiphop", name: "Hip-hop", desc: "Deep kick, clear vocals", group: "Tone", fx: { eq: [5, 4, 1, 2, -1, -1, 1, 0, 1, 2] } },
  { id: "electronic", name: "Electronic", desc: "Sub bass and sparkle", group: "Tone", fx: { eq: [4, 3, 1, 0, -2, 1, 0, 1, 3, 4] } },
  { id: "acoustic", name: "Acoustic", desc: "Warm and natural", group: "Tone", fx: { eq: [3, 3, 2, 1, 1, 1, 2, 2, 2, 1] } },
  { id: "nightcore", name: "Nightcore", desc: "Faster and higher", group: "Mood", fx: { tempo: 1.25, vinyl: true, eq: [2, 1, 0, 0, 0, 0, 1, 2, 2, 2] } },
  { id: "slowed", name: "Slowed + reverb", desc: "Slower, deeper, roomy", group: "Mood", fx: { tempo: 0.85, vinyl: true, reverb: 0.38, eq: [3, 2, 1, 0, 0, 0, -1, -2, -2, -3] } },
  { id: "vaporwave", name: "Vaporwave", desc: "Slow, dreamy, muffled", group: "Mood", fx: { tempo: 0.8, vinyl: true, lowpass: 9000, reverb: 0.22 } },
  { id: "lofi", name: "Lo-fi", desc: "Warm, dusty, soft", group: "Mood", fx: { lowpass: 3600, highpass: 140, reverb: 0.14, eq: [2, 2, 1, 0, 0, -1, -2, -3, -4, -5] } },
  { id: "8d", name: "8D audio", desc: "Sound circles your head", group: "Mood", fx: { wide8d: true, reverb: 0.16 } },
  { id: "hall", name: "Concert hall", desc: "A big, live room", group: "Mood", fx: { reverb: 0.45, eq: [1, 1, 0, 0, 0, 0, 0, 1, 1, 0] } },
];

const Player = {
  audio: new Audio(),
  ctx: null,
  nodes: null,
  pitchReady: false,
  queue: [],
  index: -1,
  shuffle: false,
  repeat: "off",
  volume: 0.85,
  muted: false,
  fx: structuredClone(FX_DEFAULT),
  bypass: false,
  countedPlay: false,
  lookup: () => null,
  listenId: null,
  listenMs: 0,
  lastTick: 0,

  flushListen() {
    if (this.listenId && this.listenMs >= 1000) invoke("track_listened", { id: this.listenId, ms: Math.round(this.listenMs) }).catch(() => {});
    this.listenMs = 0;
    this.listenId = this.current?.id || null;
  },

  emit(type, extra = {}) {
    window.dispatchEvent(new CustomEvent("player", { detail: { type, ...extra } }));
  },

  get current() {
    return this.queue[this.index] || null;
  },

  async ensureGraph() {
    if (this.ctx) {
      if (this.ctx.state === "suspended") await this.ctx.resume();
      return;
    }
    const AC = window.AudioContext || window.webkitAudioContext;
    const ctx = new AC();
    this.ctx = ctx;
    const src = ctx.createMediaElementSource(this.audio);
    let pitch = null;
    try {
      await ctx.audioWorklet.addModule("pitch-worklet.js");
      pitch = new AudioWorkletNode(ctx, "pitch-shifter", { outputChannelCount: [2] });
      this.pitchReady = true;
    } catch {
      this.pitchReady = false;
    }
    const hp = ctx.createBiquadFilter();
    hp.type = "highpass";
    const lp = ctx.createBiquadFilter();
    lp.type = "lowpass";
    const eq = EQ_BANDS.map((f, i) => {
      const b = ctx.createBiquadFilter();
      b.type = i === 0 ? "lowshelf" : i === EQ_BANDS.length - 1 ? "highshelf" : "peaking";
      b.frequency.value = f;
      b.Q.value = 1.1;
      return b;
    });
    const dry = ctx.createGain();
    const wet = ctx.createGain();
    const verb = ctx.createConvolver();
    verb.buffer = this.impulse(ctx, 2.6, 2.4);
    const sum = ctx.createGain();
    const pan = ctx.createStereoPanner();
    const lfo = ctx.createOscillator();
    lfo.frequency.value = 0.11;
    const lfoDepth = ctx.createGain();
    lfoDepth.gain.value = 0;
    lfo.connect(lfoDepth).connect(pan.pan);
    lfo.start();
    const vol = ctx.createGain();
    const analyser = ctx.createAnalyser();
    analyser.fftSize = 256;
    let head = src;
    if (pitch) head = head.connect(pitch);
    head = head.connect(hp).connect(lp);
    for (const b of eq) head = head.connect(b);
    head.connect(dry).connect(sum);
    head.connect(verb).connect(wet).connect(sum);
    sum.connect(pan).connect(vol).connect(analyser).connect(ctx.destination);
    this.nodes = { pitch, hp, lp, eq, dry, wet, lfoDepth, vol, analyser };
    this.applyFx({});
    this.setVolume(this.volume);
  },

  impulse(ctx, seconds, decay) {
    const len = Math.floor(ctx.sampleRate * seconds);
    const buf = ctx.createBuffer(2, len, ctx.sampleRate);
    for (let c = 0; c < 2; c++) {
      const d = buf.getChannelData(c);
      for (let i = 0; i < len; i++) d[i] = (Math.random() * 2 - 1) * Math.pow(1 - i / len, decay);
    }
    return buf;
  },

  applyFx(partial, presetId) {
    this.fx = { ...this.fx, ...partial };
    if (presetId !== undefined) this.fx.preset = presetId;
    const fx = this.bypass ? FX_DEFAULT : this.fx;
    this.audio.playbackRate = fx.tempo;
    this.audio.preservesPitch = !fx.vinyl;
    this.audio.webkitPreservesPitch = !fx.vinyl;
    const n = this.nodes;
    if (n) {
      const t = this.ctx.currentTime;
      const ratio = fx.vinyl ? 1 : Math.pow(2, fx.pitch / 12);
      n.pitch?.parameters.get("ratio").setValueAtTime(ratio, t);
      n.hp.frequency.setTargetAtTime(fx.highpass, t, 0.02);
      n.lp.frequency.setTargetAtTime(fx.lowpass, t, 0.02);
      n.eq.forEach((b, i) => b.gain.setTargetAtTime(fx.eq[i] || 0, t, 0.02));
      n.wet.gain.setTargetAtTime(fx.reverb * 0.9, t, 0.05);
      n.dry.gain.setTargetAtTime(1 - fx.reverb * 0.45, t, 0.05);
      n.lfoDepth.gain.setTargetAtTime(fx.wide8d ? 0.85 : 0, t, 0.1);
    }
    this.persist();
    this.emit("fx");
  },

  setBypass(on) {
    this.bypass = on;
    this.applyFx({});
  },

  usePreset(p) {
    this.applyFx({ ...structuredClone(FX_DEFAULT), ...structuredClone(p.fx) }, p.id);
  },

  resetFx() {
    this.applyFx(structuredClone(FX_DEFAULT), "flat");
  },

  setVolume(v) {
    this.volume = Math.max(0, Math.min(1, v));
    const level = this.muted ? 0 : this.volume * this.volume;
    if (this.nodes) {
      this.audio.volume = 1;
      this.nodes.vol.gain.setTargetAtTime(level, this.ctx.currentTime, 0.015);
    } else {
      this.audio.volume = level;
    }
    this.persist();
    this.emit("volume");
  },

  toggleMute() {
    this.muted = !this.muted;
    this.setVolume(this.volume);
  },

  async load(i, autoplay = true, startAt = 0) {
    if (i < 0 || i >= this.queue.length) return;
    this.flushListen();
    this.index = i;
    const t = this.current;
    this.listenId = t.id;
    this.countedPlay = false;
    this.audio.src = mediaUrl("t", t.id);
    if (startAt) this.audio.addEventListener("loadedmetadata", () => (this.audio.currentTime = startAt), { once: true });
    this.updateSession();
    this.emit("track");
    this.persist();
    if (autoplay) await this.play();
  },

  async play() {
    if (!this.current) return;
    try {
      await this.ensureGraph();
      await this.audio.play();
    } catch (e) {
      if (e?.name === "NotAllowedError") toast("Click play to start audio.", "err");
    }
  },

  pause() {
    this.audio.pause();
  },

  toggle() {
    if (!this.current) return;
    this.audio.paused ? this.play() : this.pause();
  },

  async setQueue(tracks, start = 0, shuffle = null) {
    if (!tracks.length) return;
    if (shuffle !== null) this.shuffle = shuffle;
    let list = [...tracks];
    let first = start;
    if (this.shuffle) {
      const pick = list.splice(start, 1)[0];
      for (let i = list.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [list[i], list[j]] = [list[j], list[i]];
      }
      list = [pick, ...list];
      first = 0;
    }
    this.queue = list;
    this.emit("queue");
    await this.load(first);
  },

  playNext(tracks) {
    if (!this.current) return this.setQueue(tracks);
    this.queue.splice(this.index + 1, 0, ...tracks);
    this.emit("queue");
    this.persist();
  },

  addToQueue(tracks) {
    if (!this.current) return this.setQueue(tracks);
    this.queue.push(...tracks);
    this.emit("queue");
    this.persist();
  },

  removeAt(i) {
    if (i === this.index) return;
    this.queue.splice(i, 1);
    if (i < this.index) this.index--;
    this.emit("queue");
    this.persist();
  },

  clearUpNext() {
    this.queue = this.current ? [this.current] : [];
    this.index = this.current ? 0 : -1;
    this.emit("queue");
    this.persist();
  },

  async next(auto = false) {
    if (!this.queue.length) return;
    if (auto && this.repeat === "one") {
      this.audio.currentTime = 0;
      return this.play();
    }
    if (this.index + 1 < this.queue.length) return this.load(this.index + 1);
    if (this.repeat === "all") return this.load(0);
    if (auto) {
      this.audio.currentTime = 0;
      this.pause();
      this.emit("state");
    }
  },

  async prev() {
    if (this.audio.currentTime > 3 || this.index === 0) {
      this.audio.currentTime = 0;
      return;
    }
    await this.load(this.index - 1);
  },

  seek(seconds) {
    if (!isFinite(this.audio.duration)) return;
    this.audio.currentTime = Math.max(0, Math.min(this.audio.duration, seconds));
  },

  toggleShuffle() {
    this.shuffle = !this.shuffle;
    if (this.current && this.shuffle) {
      const cur = this.current;
      const rest = this.queue.filter((_, i) => i !== this.index);
      for (let i = rest.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [rest[i], rest[j]] = [rest[j], rest[i]];
      }
      this.queue = [cur, ...rest];
      this.index = 0;
      this.emit("queue");
    }
    this.persist();
    this.emit("state");
  },

  cycleRepeat() {
    this.repeat = { off: "all", all: "one", one: "off" }[this.repeat];
    this.persist();
    this.emit("state");
  },

  updateSession() {
    const t = this.current;
    if (!t || !("mediaSession" in navigator)) return;
    const art = artUrl(t.art);
    navigator.mediaSession.metadata = new MediaMetadata({ title: t.title, artist: t.artist, album: t.album, artwork: art ? [{ src: art, sizes: "600x600", type: "image/jpeg" }] : [] });
  },

  persist() {
    clearTimeout(this._persist);
    this._persist = setTimeout(() => {
      try {
        localStorage.setItem("atmo.player", JSON.stringify({ queue: this.queue.map((t) => t.id), index: this.index, time: this.audio.currentTime || 0, volume: this.volume, shuffle: this.shuffle, repeat: this.repeat, fx: this.fx, bypass: this.bypass }));
      } catch {}
    }, 400);
  },

  restore(lookup) {
    this.lookup = lookup;
    let saved = null;
    try {
      saved = JSON.parse(localStorage.getItem("atmo.player") || "null");
    } catch {}
    if (!saved) return;
    this.volume = saved.volume ?? this.volume;
    this.shuffle = !!saved.shuffle;
    this.repeat = saved.repeat || "off";
    this.fx = { ...structuredClone(FX_DEFAULT), ...(saved.fx || {}) };
    this.bypass = !!saved.bypass;
    this.queue = (saved.queue || []).map(lookup).filter(Boolean);
    if (this.queue.length) this.load(Math.min(Math.max(0, saved.index), this.queue.length - 1), false, saved.time || 0);
    this.applyFx({});
    this.setVolume(this.volume);
    this.emit("queue");
  },

  refreshTracks(lookup) {
    this.lookup = lookup;
    const cur = this.current?.id;
    const gone = cur && !lookup(cur);
    const before = this.index;
    const kept = [];
    let nextIndex = -1;
    this.queue.forEach((t, i) => {
      const fresh = lookup(t.id);
      if (fresh) {
        if (i === before) nextIndex = kept.length;
        if (gone && nextIndex < 0 && i > before) nextIndex = kept.length;
        kept.push(fresh);
      }
    });
    this.queue = kept;
    if (gone) {
      const wasPlaying = !this.audio.paused;
      this.audio.pause();
      this.audio.removeAttribute("src");
      this.audio.load();
      if (nextIndex >= 0) this.load(nextIndex, wasPlaying);
      else this.index = -1;
    } else {
      this.index = cur ? this.queue.findIndex((t) => t.id === cur) : this.index;
    }
    this.emit("queue");
    this.emit("track");
    this.persist();
  },
};

(() => {
  const a = Player.audio;
  a.preload = "auto";
  a.crossOrigin = "anonymous";
  a.addEventListener("play", () => Player.emit("state"));
  a.addEventListener("pause", () => Player.emit("state"));
  a.addEventListener("ended", () => { Player.flushListen(); Player.next(true); });
  a.addEventListener("playing", () => (Player.lastTick = performance.now()));
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "hidden") { Player.flushListen(); } });
  window.addEventListener("beforeunload", () => Player.flushListen());
  let streak = 0;
  a.addEventListener("playing", () => (streak = 0));
  a.addEventListener("error", () => {
    if (!a.src || !Player.current) return;
    streak++;
    if (streak === 1) toast(`Couldn't play “${Player.current.title}”. The file may have moved or be unreadable.`, "err");
    if (streak >= 3) {
      toast("Several songs in a row couldn't be played, so playback stopped. Try Settings → Library → Rescan.", "err");
      return;
    }
    if (Player.index + 1 < Player.queue.length) Player.next(true);
  });
  a.addEventListener("timeupdate", () => {
    Player.emit("time");
    const nowTick = performance.now();
    if (!a.paused && Player.lastTick) {
      const d = nowTick - Player.lastTick;
      if (d > 0 && d < 2000) Player.listenMs += d;
    }
    Player.lastTick = nowTick;
    const t = Player.current;
    if (t && !Player.countedPlay && a.currentTime > Math.min(30, (a.duration || 60) * 0.5)) {
      Player.countedPlay = true;
      invoke("track_played", { id: t.id }).catch(() => {});
      window.dispatchEvent(new CustomEvent("played", { detail: t.id }));
    }
    if (Math.floor(a.currentTime) % 5 === 0) Player.persist();
  });
  a.addEventListener("loadedmetadata", () => Player.emit("time"));
  if ("mediaSession" in navigator) {
    const ms = navigator.mediaSession;
    ms.setActionHandler("play", () => Player.play());
    ms.setActionHandler("pause", () => Player.pause());
    ms.setActionHandler("previoustrack", () => Player.prev());
    ms.setActionHandler("nexttrack", () => Player.next());
    try {
      ms.setActionHandler("seekto", (d) => Player.seek(d.seekTime));
    } catch {}
  }
})();
