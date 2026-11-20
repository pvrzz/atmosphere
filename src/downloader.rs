// atmosphere · downloader.rs · Ordered download queue driving yt-dlp: format choice, progress, cancellation, tagging and history.
use crate::engine::{self, Tools};
use crate::history::{History, Record};
use crate::library::Library;
use crate::media::{self, Entry};
use crate::settings::{snap_bitrate, SaveAs, Settings};
use crate::tagging::{self, TagFields};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

pub const EVENT: &str = "atmosphere://job";
const EMIT_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Trim {
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    pub entry: Entry,
    #[serde(default)]
    pub meta: TagFields,
    pub folder: Option<String>,
    #[serde(default)]
    pub trim: Option<Trim>,
    #[serde(default)]
    pub playlist: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOptions {
    pub save_as: SaveAs,
    pub mp3_bitrate: u32,
    pub video_height: u32,
    pub video_container: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub id: u64,
    pub state: &'static str,
    pub progress: f64,
    pub received: u64,
    pub total: Option<u64>,
    pub speed: u64,
    pub stage: u32,
    pub format: Option<String>,
    pub message: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

struct Pending {
    id: u64,
    req: JobRequest,
    save: SaveOptions,
    flag: Arc<AtomicBool>,
}

struct Queue {
    pending: VecDeque<Pending>,
    running: usize,
    limit: usize,
}

pub struct Downloader {
    app: AppHandle,
    history: Arc<History>,
    library: Arc<Library>,
    http: reqwest::Client,
    settings: Arc<Mutex<Settings>>,
    queue: Mutex<Queue>,
    flags: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    next: AtomicU64,
}

pub const LIBRARY_EVENT: &str = "atmosphere://library";

async fn after_download(app: &AppHandle, library: &Library, http: &reqwest::Client, settings: &Settings, req: &JobRequest, path: &Path) {
    let playlists = PathBuf::from(&settings.playlists_dir);
    let attached = req.meta.lyrics.trim();
    if !attached.is_empty() {
        if crate::lyrics::is_timed(attached) {
            let _ = tokio::fs::write(path.with_extension("lrc"), attached).await;
        }
    } else if settings.lyrics_on_download && tagging_ext(path) {
        let q = crate::lyrics::Query {
            title: if req.meta.title.trim().is_empty() { req.entry.title.clone() } else { req.meta.title.clone() },
            artist: if req.meta.artist.trim().is_empty() { req.entry.artist.clone() } else { req.meta.artist.clone() },
            album: req.meta.album.clone(),
            duration_ms: req.entry.duration_ms,
            path: None,
        };
        if let Ok(Some(l)) = crate::lyrics::find(http, settings, &q, false).await.map(|f| f.lyrics) {
            if l.synced {
                let _ = tokio::fs::write(path.with_extension("lrc"), crate::lyrics::to_lrc(&l)).await;
            }
        }
    }
    let _ = library.add_file(path, &playlists);
    if let Some(name) = req.playlist.as_deref().filter(|n| !n.trim().is_empty()) {
        let _ = library.playlist_add(&playlists, name, &[path.to_path_buf()], !settings.playlist_copy);
    }
    let _ = app.emit(LIBRARY_EVENT, serde_json::json!({ "type": "changed" }));
}

fn tagging_ext(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| crate::library::AUDIO_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

impl Downloader {
    pub fn new(app: AppHandle, settings: Arc<Mutex<Settings>>, history: Arc<History>, library: Arc<Library>, http: reqwest::Client) -> Self {
        let limit = settings.lock().unwrap().concurrency.max(1) as usize;
        Self {
            app,
            history,
            library,
            http,
            settings,
            queue: Mutex::new(Queue { pending: VecDeque::new(), running: 0, limit }),
            flags: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    pub fn set_concurrency(self: &Arc<Self>, n: u8) {
        self.queue.lock().unwrap().limit = n.max(1) as usize;
        self.pump();
    }

    pub fn enqueue(self: &Arc<Self>, reqs: Vec<JobRequest>, save: SaveOptions) -> Vec<u64> {
        let mut ids = Vec::with_capacity(reqs.len());
        {
            let mut q = self.queue.lock().unwrap();
            let mut flags = self.flags.lock().unwrap();
            for req in reqs {
                let id = self.next.fetch_add(1, Ordering::SeqCst);
                let flag = Arc::new(AtomicBool::new(false));
                flags.insert(id, flag.clone());
                q.pending.push_back(Pending { id, req, save: save.clone(), flag });
                ids.push(id);
            }
        }
        self.pump();
        ids
    }

    fn pump(self: &Arc<Self>) {
        loop {
            let next = {
                let mut q = self.queue.lock().unwrap();
                if q.running >= q.limit {
                    None
                } else {
                    let p = q.pending.pop_front();
                    if p.is_some() {
                        q.running += 1;
                    }
                    p
                }
            };
            let Some(p) = next else { break };
            let this = self.clone();
            tauri::async_runtime::spawn(async move {
                let settings = this.settings.lock().unwrap().clone();
                let mut job = Job::new(this.app.clone(), p.id, p.flag.clone());
                let result = match job.check() {
                    Ok(()) => job.run(&settings, &p.req, &p.save).await,
                    Err(e) => Err(e),
                };
                match result {
                    Ok(Some(path)) => {
                        this.history.add(Record::new(&p.req, &path, job.format.clone()));
                        after_download(&this.app, &this.library, &this.http, &settings, &p.req, &path).await;
                    }
                    Ok(None) => {}
                    Err(e) if e.is::<Cancelled>() => job.emit("cancelled", None, None),
                    Err(e) => job.emit("error", Some(format!("{e:#}")), None),
                }
                this.flags.lock().unwrap().remove(&p.id);
                let idle = {
                    let mut q = this.queue.lock().unwrap();
                    q.running -= 1;
                    q.running == 0 && q.pending.is_empty()
                };
                if idle {
                    let s = this.settings.lock().unwrap().clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || crate::storage::clear_stalled(&s)).await;
                }
                this.pump();
            });
        }
    }

    fn drop_pending(&self, pred: impl Fn(&Pending) -> bool) {
        let removed: Vec<Pending> = {
            let mut q = self.queue.lock().unwrap();
            let (gone, keep): (Vec<_>, Vec<_>) = q.pending.drain(..).partition(|p| pred(p));
            q.pending = keep.into();
            gone
        };
        let mut flags = self.flags.lock().unwrap();
        for p in removed {
            flags.remove(&p.id);
            Job::new(self.app.clone(), p.id, p.flag).emit("cancelled", None, None);
        }
    }

    pub fn cancel(&self, id: u64) {
        if let Some(f) = self.flags.lock().unwrap().get(&id) {
            f.store(true, Ordering::SeqCst);
        }
        self.drop_pending(|p| p.id == id);
    }

    pub fn busy(&self) -> bool {
        let q = self.queue.lock().unwrap();
        q.running > 0 || !q.pending.is_empty()
    }

    pub fn cancel_all(&self) {
        self.flags.lock().unwrap().values().for_each(|f| f.store(true, Ordering::SeqCst));
        self.drop_pending(|_| true);
    }
}

struct Job {
    app: AppHandle,
    id: u64,
    flag: Arc<AtomicBool>,
    state: &'static str,
    progress: f64,
    received: u64,
    total: Option<u64>,
    speed: u64,
    stage: u32,
    last_emit: Instant,
    format: Option<String>,
}

impl Job {
    fn new(app: AppHandle, id: u64, flag: Arc<AtomicBool>) -> Self {
        Self {
            app,
            id,
            flag,
            state: "queued",
            progress: 0.0,
            received: 0,
            total: None,
            speed: 0,
            stage: 0,
            last_emit: Instant::now(),
            format: None,
        }
    }

    fn check(&self) -> Result<()> {
        if self.flag.load(Ordering::SeqCst) {
            return Err(anyhow!(Cancelled));
        }
        Ok(())
    }

    fn emit(&mut self, state: &'static str, message: Option<String>, path: Option<&Path>) {
        self.state = state;
        self.last_emit = Instant::now();
        let _ = self.app.emit(
            EVENT,
            JobEvent {
                id: self.id,
                state,
                progress: self.progress,
                received: self.received,
                total: self.total,
                speed: self.speed,
                stage: self.stage,
                format: self.format.clone(),
                message,
                path: path.map(|p| p.to_string_lossy().into_owned()),
            },
        );
    }

    async fn run(&mut self, settings: &Settings, req: &JobRequest, save: &SaveOptions) -> Result<Option<PathBuf>> {
        self.emit("preparing", None, None);
        let mut swapped_note = None;
        let swapped;
        let req = if settings.prefer_official_audio && !matches!(save.save_as, SaveAs::Video) && req.entry.music_video && req.entry.platform == "YouTube" {
            let tools = engine::locate(&settings.ytdlp_path, &settings.ffmpeg_path)?;
            let artist = if req.meta.artist.trim().is_empty() { req.entry.artist.clone() } else { req.meta.artist.clone() };
            let title = if req.meta.title.trim().is_empty() { req.entry.title.clone() } else { req.meta.title.clone() };
            match crate::media::find_official_audio(settings, &tools, &artist, &title).await {
                Some(url) if url != req.entry.url => {
                    let mut r = req.clone();
                    r.entry.url = url;
                    r.entry.media = None;
                    swapped = r;
                    swapped_note = Some("Used the official audio upload instead of the music video's sound.".to_string());
                    &swapped
                }
                _ => {
                    swapped_note = Some("No official audio upload was found, so the music video's sound was saved.".to_string());
                    req
                }
            }
        } else {
            req
        };
        let entry = &req.entry;
        match entry.availability.as_str() {
            "drm" => bail!("This stream is DRM-encrypted, so it was skipped. If it's your own track, turn on downloads for it in the platform's track settings (or enable “Use my browser sign-in”), then choose Original file to get your uploaded master."),
            "preview" => bail!("Only a preview is available for this item, so it was skipped."),
            _ => {}
        }
        let tools = engine::locate(&settings.ytdlp_path, &settings.ffmpeg_path)?;
        let dir = target_dir(settings, req);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| anyhow!("Couldn't create the output folder ({e})"))?;
        let stem = file_stem(settings, req);
        let plan = plan(entry, save, tools.ffmpeg.is_some())?;
        self.format = Some(plan.label.clone());
        if let Some(ext) = &plan.known_ext {
            let target = dir.join(format!("{stem}.{ext}"));
            if target.exists() {
                self.progress = 1.0;
                self.emit("skipped", Some("Already in your library".into()), Some(&target));
                return Ok(None);
            }
        }
        let job_temp = engine::temp_root().join(format!("job-{}", self.id));
        let _ = tokio::fs::create_dir_all(&job_temp).await;
        let args = build_args(settings, &tools, req, &plan, &dir, &stem, &job_temp);
        let outcome = self.execute(&tools, args, &dir, &stem).await;
        let _ = tokio::fs::remove_dir_all(&job_temp).await;
        let path = match outcome? {
            Some(p) => p,
            None => {
                self.progress = 1.0;
                self.emit("skipped", Some("Downloaded before, so it was skipped. Turn off “Skip anything downloaded before” to fetch it again.".into()), None);
                return Ok(None);
            }
        };
        let mut notes = plan.notes.clone();
        notes.extend(swapped_note);
        if settings.embed_metadata {
            self.emit("tagging", None, None);
            if let Err(e) = tagging::apply(&path, &req.meta, &req.entry.url, tools.ffmpeg.as_deref()).await {
                notes.push(format!("Saved, but your metadata edits couldn't be written ({e})."));
            }
        }
        let path = if plan.video && settings.video_keep_audio {
            match (tools.ffmpeg.as_deref(), &path) {
                (Some(ff), video) => match extract_audio(ff, video).await {
                    Ok(audio) => {
                        if settings.embed_metadata {
                            let _ = tagging::apply(&audio, &req.meta, &req.entry.url, Some(ff)).await;
                        }
                        notes.push("Saved the video and a matching audio file. Play it as a song, and show the video any time.".into());
                        audio
                    }
                    Err(e) => {
                        notes.push(format!("Saved the video, but the audio file couldn't be made ({e})."));
                        path
                    }
                },
                _ => path,
            }
        } else {
            path
        };
        self.progress = 1.0;
        let note = (!notes.is_empty()).then(|| notes.join(" "));
        self.emit("done", note, Some(&path));
        Ok(Some(path))
    }

    async fn execute(&mut self, tools: &Tools, args: Vec<String>, dir: &Path, stem: &str) -> Result<Option<PathBuf>> {
        let mut child = engine::command(&tools.ytdlp)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow!("Couldn't start yt-dlp ({e})"))?;
        let (tx, mut rx) = mpsc::unbounded_channel::<(bool, String)>();
        for (is_err, pipe) in [(false, child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)), (true, child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>))] {
            if let Some(pipe) = pipe {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(pipe).lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        let _ = tx.send((is_err, l));
                    }
                });
            }
        }
        drop(tx);
        let mut final_path = None;
        let mut errors: Vec<String> = Vec::new();
        let started = Instant::now();
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some((is_err, line)) = msg else { break };
                    if is_err {
                        if line.contains("ERROR") || errors.len() < 40 { errors.push(line); }
                        continue;
                    }
                    if let Some(json) = line.strip_prefix("ATMOSP ") {
                        self.on_progress(json, started);
                    } else if let Some(p) = line.strip_prefix("ATMOSF ") {
                        final_path = Some(PathBuf::from(p.trim()));
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            }
            if self.flag.load(Ordering::SeqCst) {
                let _ = child.kill().await;
                cleanup(dir, stem).await;
                return Err(anyhow!(Cancelled));
            }
        }
        let status = child.wait().await?;
        if !status.success() {
            cleanup(dir, stem).await;
            let err = errors.iter().rev().find(|l| l.contains("ERROR")).cloned().unwrap_or_else(|| errors.join("\n"));
            bail!(media::humanize(&err));
        }
        Ok(final_path.filter(|p| p.exists()))
    }

    fn on_progress(&mut self, json: &str, started: Instant) {
        let Ok(v) = serde_json::from_str::<Value>(json) else { return };
        match v["status"].as_str() {
            Some("downloading") => {
                if self.state != "downloading" {
                    self.state = "downloading";
                    self.last_emit = Instant::now() - EMIT_INTERVAL;
                }
                self.received = v["downloaded_bytes"].as_u64().unwrap_or(self.received);
                self.total = v["total_bytes"].as_u64().or_else(|| v["total_bytes_estimate"].as_f64().map(|t| t as u64));
                self.speed = v["speed"].as_f64().map(|s| s as u64).unwrap_or_else(|| (self.received as f64 / started.elapsed().as_secs_f64().max(0.1)) as u64);
                self.progress = match (self.total, v["fragment_index"].as_u64(), v["fragment_count"].as_u64()) {
                    (Some(t), _, _) if t > 0 => (self.received as f64 / t as f64).min(1.0),
                    (_, Some(i), Some(c)) if c > 0 => i as f64 / c as f64,
                    _ => self.progress,
                };
                if self.last_emit.elapsed() >= EMIT_INTERVAL {
                    self.emit("downloading", None, None);
                }
            }
            Some("finished") => {
                self.stage += 1;
                self.progress = 1.0;
                self.emit("processing", None, None);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
struct Plan {
    format: String,
    sort: Option<String>,
    extract: Option<(String, Option<u32>)>,
    merge: Option<String>,
    video: bool,
    known_ext: Option<String>,
    label: String,
    notes: Vec<String>,
}

fn plan(entry: &Entry, save: &SaveOptions, has_ffmpeg: bool) -> Result<Plan> {
    let media = entry.media.clone().unwrap_or_default();
    let from_original = media.original && media.original_lossless;
    let source_abr = if from_original || media.lossless { None } else { media.best_abr };
    let stream = media
        .best_codec
        .as_deref()
        .map(|c| match media.best_abr {
            Some(a) => format!("{} {a} kbps", crate::media::codec_name(c)),
            None => crate::media::codec_name(c),
        })
        .unwrap_or_else(|| "best available".into());
    let original_label = format!("original {}", media.original_ext.as_deref().unwrap_or("file").to_uppercase());
    let src = if from_original { original_label.clone() } else { stream.clone() };
    let lossless_source = from_original || media.lossless;
    let source_format = if from_original { "download/bestaudio[format_id!*=drc]/bestaudio/best" } else { "bestaudio[format_id!*=drc]/bestaudio/best" };
    let mut notes = Vec::new();
    let needs_ffmpeg = |what: &str| -> Result<()> {
        if has_ffmpeg {
            Ok(())
        } else {
            bail!("{what} needs ffmpeg. Install it from Settings → Engine.")
        }
    };
    let audio = |format: &str| Plan {
        format: format.into(),
        sort: None,
        extract: has_ffmpeg.then(|| ("best".into(), None)),
        merge: None,
        video: false,
        known_ext: None,
        label: format!("Best audio · {stream}"),
        notes: Vec::new(),
    };
    Ok(match save.save_as {
        SaveAs::Original => {
            if media.original {
                let mut p = audio("download/bestaudio[format_id!*=drc]/bestaudio/best");
                p.extract = None;
                p.label = format!("Original file{}", media.original_ext.as_deref().map(|e| format!(" · {}", e.to_uppercase())).unwrap_or_default());
                p
            } else {
                let mut p = audio("bestaudio[format_id!*=drc]/bestaudio/best");
                notes.push("The uploader hasn't offered an original file, so the best stream was saved instead.".into());
                p.notes = notes;
                p
            }
        }
        SaveAs::Best => audio("bestaudio[format_id!*=drc]/bestaudio/best"),
        SaveAs::Mp3 => {
            needs_ffmpeg("Saving as MP3")?;
            let kbps = snap_bitrate(save.mp3_bitrate, source_abr);
            if kbps < save.mp3_bitrate {
                notes.push(format!("Encoded at {kbps} kbps to match the source ({src}); a higher bitrate wouldn't add quality."));
            }
            let floor = kbps.saturating_sub(8);
            Plan {
                format: if from_original { source_format.into() } else { format!("bestaudio[acodec^=mp3][abr>={floor}][format_id!*=drc]/bestaudio[format_id!*=drc]/bestaudio/best") },
                sort: None,
                extract: Some(("mp3".into(), Some(kbps))),
                merge: None,
                video: false,
                known_ext: Some("mp3".into()),
                label: format!("MP3 {kbps} kbps · from {src}"),
                notes,
            }
        }
        SaveAs::M4a => {
            needs_ffmpeg("Saving as M4A")?;
            Plan {
                format: "bestaudio[acodec^=mp4a][format_id!*=drc]/bestaudio[format_id!*=drc]/bestaudio/best".into(),
                sort: None,
                extract: Some(("m4a".into(), source_abr.map(|a| a.max(128)))),
                merge: None,
                video: false,
                known_ext: Some("m4a".into()),
                label: format!("M4A (AAC) · from {src}"),
                notes,
            }
        }
        SaveAs::Flac | SaveAs::Wav => {
            let ext = save.save_as.audio_format().unwrap_or("flac");
            needs_ffmpeg(&format!("Saving as {}", ext.to_uppercase()))?;
            if !lossless_source {
                notes.push(format!("{} is lossless, but it can't add quality beyond the {src} source.", ext.to_uppercase()));
            }
            Plan {
                format: source_format.into(),
                sort: None,
                extract: Some((ext.into(), None)),
                merge: None,
                video: false,
                known_ext: Some(ext.into()),
                label: format!("{} · from {src}", ext.to_uppercase()),
                notes,
            }
        }
        SaveAs::Video => {
            if !media.has_video && entry.detailed {
                bail!("This item has no video. Choose an audio option instead.");
            }
            needs_ffmpeg("Saving video")?;
            let available = media.heights.first().copied();
            let h = match (save.video_height, available) {
                (0, _) => None,
                (want, Some(max)) if want > max => {
                    notes.push(format!("{want}p isn't available; saved the highest available ({max}p)."));
                    Some(max)
                }
                (want, _) => Some(want),
            };
            let container = match save.video_container.as_str() {
                "mkv" | "webm" => save.video_container.clone(),
                _ => "mp4".into(),
            };
            let res = h.map(|h| format!("res:{h},")).unwrap_or_default();
            let sort = if container == "mp4" { format!("{res}vcodec:h264,acodec:aac") } else { res.trim_end_matches(',').to_string() };
            Plan {
                format: "bv*+ba/b".into(),
                sort: (!sort.is_empty()).then_some(sort),
                extract: None,
                merge: Some(container.clone()),
                video: true,
                known_ext: Some(container.clone()),
                label: format!("Video {} · {}", h.map(|h| format!("{h}p")).unwrap_or_else(|| "best".into()), container.to_uppercase()),
                notes,
            }
        }
    })
}

fn template_escape(s: &str) -> String {
    s.replace('%', "%%")
}

fn build_args(settings: &Settings, tools: &Tools, req: &JobRequest, plan: &Plan, dir: &Path, stem: &str, temp: &Path) -> Vec<String> {
    let mut a = media::base_args(settings, tools);
    let push = |a: &mut Vec<String>, items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    push(&mut a, &["--newline", "--progress", "--no-playlist", "--no-mtime", "--no-overwrites", "--concurrent-fragments", "4"]);
    push(&mut a, &["--progress-template", "download:ATMOSP %(progress)j", "--print", "after_move:ATMOSF %(filepath)s"]);
    push(&mut a, &["-P", &dir.to_string_lossy(), "-P", &format!("temp:{}", temp.to_string_lossy()), "-o", &format!("{}.%(ext)s", template_escape(stem))]);
    push(&mut a, &["-f", &plan.format]);
    if let Some(sort) = &plan.sort {
        push(&mut a, &["-S", sort]);
    }
    if let Some((fmt, kbps)) = &plan.extract {
        push(&mut a, &["-x", "--audio-format", fmt]);
        if let Some(k) = kbps {
            push(&mut a, &["--audio-quality", &format!("{k}K")]);
        }
    }
    if let Some(container) = &plan.merge {
        push(&mut a, &["--merge-output-format", container, "--remux-video", container]);
    }
    let has_ff = tools.ffmpeg.is_some();
    if settings.embed_metadata {
        push(&mut a, &["--embed-metadata"]);
        let wav = plan.extract.as_ref().is_some_and(|(f, _)| f == "wav");
        if has_ff && !wav {
            push(&mut a, &["--embed-thumbnail", "--convert-thumbnails", "jpg"]);
        }
    }
    if settings.embed_chapters && has_ff {
        push(&mut a, &["--embed-chapters"]);
    }
    if plan.video && settings.embed_subtitles && has_ff {
        push(&mut a, &["--write-subs", "--sub-langs", settings.subtitle_langs.trim(), "--embed-subs"]);
    }
    if settings.sponsorblock && has_ff && req.entry.platform == "YouTube" {
        let cats = if plan.video { "sponsor,selfpromo,interaction" } else { "sponsor,selfpromo,interaction,music_offtopic" };
        push(&mut a, &["--sponsorblock-remove", cats]);
    }
    if let Some(t) = req.trim.as_ref().filter(|t| !t.start.trim().is_empty() || !t.end.trim().is_empty()) {
        let start = if t.start.trim().is_empty() { "0" } else { t.start.trim() };
        let end = if t.end.trim().is_empty() { "inf" } else { t.end.trim() };
        push(&mut a, &["--download-sections", &format!("*{start}-{end}")]);
        if has_ff {
            push(&mut a, &["--force-keyframes-at-cuts"]);
        }
    }
    if settings.skip_downloaded {
        push(&mut a, &["--download-archive", &engine::data_dir().join("archive.txt").to_string_lossy()]);
    }
    if !settings.rate_limit.trim().is_empty() {
        push(&mut a, &["--limit-rate", settings.rate_limit.trim()]);
    }
    push(&mut a, &["--", &req.entry.url]);
    a
}

async fn cleanup(dir: &Path, stem: &str) {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else { return };
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().into_owned();
        let leftover = [".part", ".ytdl", ".temp", ".webp", ".jpg", ".png"].iter().any(|x| name.ends_with(x)) || name.contains(".part-Frag");
        if name.starts_with(stem) && leftover {
            let _ = tokio::fs::remove_file(e.path()).await;
        }
    }
}

pub async fn extract_audio(ffmpeg: &Path, video: &Path) -> Result<PathBuf> {
    let mp4 = video.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("m4v") || e.eq_ignore_ascii_case("mov"));
    let attempts: Vec<(&str, Vec<&str>)> = if mp4 {
        vec![("m4a", vec!["-c:a", "copy"]), ("m4a", vec!["-c:a", "aac", "-b:a", "256k"])]
    } else {
        vec![("opus", vec!["-c:a", "copy"]), ("m4a", vec!["-c:a", "aac", "-b:a", "256k"])]
    };
    for (ext, codec) in attempts {
        let out = video.with_extension(ext);
        let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-i"].map(String::from).into();
        args.push(video.to_string_lossy().into_owned());
        args.extend(["-map", "0:a:0", "-vn", "-map_metadata", "0"].map(String::from));
        args.extend(codec.iter().map(|c| c.to_string()));
        args.push(out.to_string_lossy().into_owned());
        let ok = engine::command(ffmpeg).args(&args).stdout(Stdio::null()).stderr(Stdio::null()).status().await.is_ok_and(|s| s.success());
        if ok && out.metadata().is_ok_and(|m| m.len() > 0) {
            return Ok(out);
        }
        let _ = tokio::fs::remove_file(&out).await;
    }
    bail!("ffmpeg couldn't read the video's audio")
}

pub fn target_dir(settings: &Settings, req: &JobRequest) -> PathBuf {
    let base = PathBuf::from(&settings.output_dir);
    let named = req.folder.as_deref().map(str::trim).filter(|n| !n.is_empty() && settings.collection_folders);
    match (req.kind.as_deref(), named) {
        (Some("album"), Some(name)) => base.join("Albums").join(sanitize(name)),
        (Some("playlist"), Some(name)) => {
            let lists = PathBuf::from(&settings.playlists_dir);
            if settings.playlists_dir.trim().is_empty() { base.join("Playlists") } else { lists }.join(sanitize(name))
        }
        (None, Some(name)) => base.join(sanitize(name)),
        _ => {
            let album = req.meta.album.trim();
            let existing = (settings.collection_folders && !album.is_empty()).then(|| existing_dir(&base.join("Albums"), &sanitize(album))).flatten();
            existing.unwrap_or_else(|| base.join("Tracks"))
        }
    }
}

pub fn existing_dir(parent: &Path, name: &str) -> Option<PathBuf> {
    let want = name.to_lowercase();
    std::fs::read_dir(parent).ok()?.flatten().find(|e| e.path().is_dir() && e.file_name().to_string_lossy().to_lowercase() == want).map(|e| e.path())
}

fn file_stem(settings: &Settings, req: &JobRequest) -> String {
    let m = &req.meta;
    let title = if m.title.trim().is_empty() { &req.entry.title } else { &m.title };
    let artist = if m.artist.trim().is_empty() { &req.entry.artist } else { &m.artist };
    let width = m.track_total.map_or(2, |n| n.to_string().len().max(2));
    let index = m.track_number.map(|i| format!("{i:0width$}"));
    let year = m.date.get(..4).unwrap_or_default().to_string();
    let template = &settings.filename_template;
    let mut name = template
        .replace("{artist}", artist)
        .replace("{title}", title)
        .replace("{album}", &m.album)
        .replace("{albumartist}", &m.album_artist)
        .replace("{collection}", req.folder.as_deref().unwrap_or(""))
        .replace("{genre}", &m.genre)
        .replace("{year}", &year)
        .replace("{platform}", &req.entry.platform)
        .replace("{index}", index.as_deref().unwrap_or(""));
    if settings.number_tracks && req.folder.is_some() && !template.contains("{index}") {
        if let Some(i) = index {
            name = format!("{i} - {name}");
        }
    }
    sanitize(&name)
}

pub fn sanitize(input: &str) -> String {
    let cleaned: String = input.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let mut out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    out = out.trim_matches(|c: char| c == '.' || c == ' ' || c == '-').to_string();
    if out.chars().count() > 150 {
        out = out.chars().take(150).collect::<String>().trim_end().to_string();
    }
    let upper = out.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT")) && upper.len() == 4 && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    if out.is_empty() {
        "untitled".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaInfo;

    fn entry(abr: Option<u32>, lossless: bool, heights: Vec<u32>) -> Entry {
        Entry {
            title: "Night Drive".into(),
            artist: "Nova".into(),
            url: "https://example.com/x".into(),
            detailed: true,
            availability: "ok".into(),
            media: Some(MediaInfo { best_abr: abr, best_codec: Some("aac".into()), lossless, has_video: !heights.is_empty(), heights, ..Default::default() }),
            ..Default::default()
        }
    }

    fn save(save_as: SaveAs, kbps: u32, h: u32) -> SaveOptions {
        SaveOptions { save_as, mp3_bitrate: kbps, video_height: h, video_container: "mp4".into() }
    }

    #[test]
    fn mp3_bitrate_never_exceeds_source() {
        let p = plan(&entry(Some(160), false, vec![]), &save(SaveAs::Mp3, 320, 0), true).unwrap();
        assert_eq!(p.extract, Some(("mp3".into(), Some(160))));
        assert!(p.label.starts_with("MP3 160 kbps"));
        assert_eq!(p.notes.len(), 1);
        let p = plan(&entry(None, true, vec![]), &save(SaveAs::Mp3, 320, 0), true).unwrap();
        assert_eq!(p.extract, Some(("mp3".into(), Some(320))));
    }

    #[test]
    fn lossless_original_lifts_the_mp3_cap_honestly() {
        let mut e = entry(Some(160), false, vec![]);
        if let Some(m) = e.media.as_mut() {
            m.original = true;
            m.original_lossless = true;
            m.original_ext = Some("wav".into());
        }
        let p = plan(&e, &save(SaveAs::Mp3, 320, 0), true).unwrap();
        assert_eq!(p.extract, Some(("mp3".into(), Some(320))));
        assert!(p.format.starts_with("download/"));
        assert!(p.label.contains("original WAV"));
        let plain = plan(&entry(Some(160), false, vec![]), &save(SaveAs::Flac, 320, 0), true).unwrap();
        assert_eq!(plain.notes.len(), 1);
    }

    #[test]
    fn video_plans_respect_available_heights() {
        let p = plan(&entry(Some(128), false, vec![720, 480]), &save(SaveAs::Video, 1080, 1080), true).unwrap();
        assert!(p.label.contains("720p"));
        assert_eq!(p.sort.as_deref(), Some("res:720,vcodec:h264,acodec:aac"));
        assert!(plan(&entry(Some(128), false, vec![]), &save(SaveAs::Video, 0, 1080), true).is_err());
        assert!(plan(&entry(Some(128), false, vec![720]), &save(SaveAs::Mp3, 320, 0), false).is_err());
    }

    #[tokio::test]
    async fn keeps_an_audio_file_beside_a_video_and_pairs_them() {
        let Some(ff) = engine::find_tool("ffmpeg") else { return };
        let dir = std::env::temp_dir().join(format!("atmo-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("Westwood - Eyes.mp4");
        let made = engine::command(&ff)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=160x90:rate=10", "-f", "lavfi", "-i", "sine=frequency=440", "-t", "2", "-c:v", "libx264", "-c:a", "aac", "-shortest"])
            .arg(&video)
            .status()
            .await
            .is_ok_and(|s| s.success());
        if !made {
            return;
        }
        let audio = extract_audio(&ff, &video).await.unwrap();
        assert_eq!(audio.extension().unwrap(), "m4a");
        assert!(crate::library::is_playable(&audio));
        assert!(!crate::library::is_playable(&video));
        let t = crate::library::read_track(&audio, None).unwrap();
        assert_eq!(t.video.as_deref(), Some(&*video.to_string_lossy()));
        std::fs::remove_file(&audio).unwrap();
        assert!(crate::library::is_playable(&video));
        let v = crate::library::read_track(&video, None).unwrap();
        assert_eq!(v.video.as_deref(), Some(&*video.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sorts_downloads_into_albums_playlists_and_tracks() {
        let settings = Settings { output_dir: "M".into(), playlists_dir: "M/Playlists".into(), collection_folders: true, ..Default::default() };
        let req = |kind: Option<&str>, folder: Option<&str>| JobRequest { entry: entry(None, false, vec![]), meta: TagFields::default(), folder: folder.map(String::from), trim: None, playlist: None, kind: kind.map(String::from) };
        assert_eq!(target_dir(&settings, &req(Some("album"), Some("Luxury"))), PathBuf::from("M").join("Albums").join("Luxury"));
        assert_eq!(target_dir(&settings, &req(Some("playlist"), Some("Late night"))), PathBuf::from("M/Playlists").join("Late night"));
        assert_eq!(target_dir(&settings, &req(Some("track"), None)), PathBuf::from("M").join("Tracks"));
        let flat = Settings { collection_folders: false, ..settings.clone() };
        assert_eq!(target_dir(&flat, &req(Some("album"), Some("Luxury"))), PathBuf::from("M").join("Tracks"));
    }

    #[test]
    fn single_songs_join_an_album_folder_that_already_exists() {
        let base = std::env::temp_dir().join(format!("atmo-single-{}", std::process::id()));
        std::fs::create_dir_all(base.join("Albums").join("Luxury")).unwrap();
        let settings = Settings { output_dir: base.to_string_lossy().into_owned(), collection_folders: true, ..Default::default() };
        let req = |album: &str| JobRequest { entry: entry(None, false, vec![]), meta: TagFields { album: album.into(), ..Default::default() }, folder: None, trim: None, playlist: None, kind: Some("track".into()) };
        assert_eq!(target_dir(&settings, &req("luxury")), base.join("Albums").join("Luxury"));
        assert_eq!(target_dir(&settings, &req("Something Else")), base.join("Tracks"));
        assert_eq!(target_dir(&settings, &req("")), base.join("Tracks"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn builds_stems_and_escapes_templates() {
        let settings = Settings { output_dir: "out".into(), ..Default::default() };
        let meta = TagFields { title: "100% Night".into(), track_number: Some(3), track_total: Some(120), ..Default::default() };
        let req = JobRequest { entry: entry(None, false, vec![]), meta, folder: Some("Nova - Glass: Vol 1".into()), trim: None, playlist: None, kind: None };
        let stem = file_stem(&settings, &req);
        assert_eq!(stem, "003 - Nova - 100% Night");
        assert_eq!(template_escape(&stem), "003 - Nova - 100%% Night");
        assert_eq!(target_dir(&settings, &req), PathBuf::from("out").join("Nova - Glass_ Vol 1"));
        assert_eq!(sanitize("con"), "_con");
    }
}
