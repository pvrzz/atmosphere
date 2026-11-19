// atmosphere · settings.rs · Persistent user settings: rights acknowledgment, save-as defaults, engine paths and network options.
use crate::lyrics::{default_providers, CustomProvider, ProviderCfg};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const TERMS_VERSION: u32 = 1;
pub const MP3_BITRATES: [u32; 7] = [64, 96, 128, 160, 192, 256, 320];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveAs {
    Original,
    #[default]
    Best,
    Mp3,
    M4a,
    Flac,
    Wav,
    Video,
}

impl SaveAs {
    pub fn audio_format(self) -> Option<&'static str> {
        match self {
            SaveAs::Mp3 => Some("mp3"),
            SaveAs::M4a => Some("m4a"),
            SaveAs::Flac => Some("flac"),
            SaveAs::Wav => Some("wav"),
            _ => None,
        }
    }
}

pub fn snap_bitrate(requested: u32, cap: Option<u32>) -> u32 {
    let limit = cap.map_or(requested, |c| requested.min(c));
    MP3_BITRATES.iter().copied().filter(|b| *b <= limit).max().unwrap_or(MP3_BITRATES[0])
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub rights_version: u32,
    pub rights_accepted_at: Option<u64>,
    pub output_dir: String,
    pub save_as: SaveAs,
    pub mp3_bitrate: u32,
    pub video_height: u32,
    pub video_container: String,
    pub video_keep_audio: bool,
    pub concurrency: u8,
    pub filename_template: String,
    pub embed_metadata: bool,
    pub collection_folders: bool,
    pub number_tracks: bool,
    pub skip_downloaded: bool,
    pub sponsorblock: bool,
    pub embed_subtitles: bool,
    pub subtitle_langs: String,
    pub embed_chapters: bool,
    pub cookies_browser: String,
    pub cookies_file: String,
    pub api_enabled: bool,
    pub check_updates: bool,
    pub artist_info: bool,
    pub api_port: u16,
    pub api_read: bool,
    pub api_control: bool,
    pub api_links: bool,
    pub proxy: String,
    pub rate_limit: String,
    pub ytdlp_path: String,
    pub ffmpeg_path: String,
    pub watch_clipboard: bool,
    pub theme: String,
    pub library_folders: Vec<String>,
    pub playlists_dir: String,
    pub playlist_copy: bool,
    pub lyrics_providers: Vec<ProviderCfg>,
    pub musixmatch_key: String,
    pub custom_lyrics: Vec<CustomProvider>,
    pub lyrics_on_download: bool,
    pub fx_presets: Vec<serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        let base = dirs::audio_dir().or_else(dirs::download_dir).unwrap_or_else(|| PathBuf::from("."));
        let out = base.join("Atmosphere");
        Self {
            rights_version: 0,
            rights_accepted_at: None,
            output_dir: out.to_string_lossy().into_owned(),
            save_as: SaveAs::Best,
            mp3_bitrate: 320,
            video_height: 1080,
            video_container: "mp4".into(),
            video_keep_audio: true,
            concurrency: 3,
            filename_template: "{artist} - {title}".into(),
            embed_metadata: true,
            collection_folders: true,
            number_tracks: true,
            skip_downloaded: true,
            sponsorblock: false,
            embed_subtitles: false,
            subtitle_langs: "en".into(),
            embed_chapters: true,
            cookies_browser: String::new(),
            cookies_file: String::new(),
            api_enabled: false,
            check_updates: true,
            artist_info: true,
            api_port: 47823,
            api_read: true,
            api_control: true,
            api_links: false,
            proxy: String::new(),
            rate_limit: String::new(),
            ytdlp_path: String::new(),
            ffmpeg_path: String::new(),
            watch_clipboard: true,
            theme: "system".into(),
            library_folders: vec![out.to_string_lossy().into_owned()],
            playlists_dir: out.join("Playlists").to_string_lossy().into_owned(),
            playlist_copy: false,
            lyrics_providers: default_providers(),
            musixmatch_key: String::new(),
            custom_lyrics: Vec::new(),
            lyrics_on_download: false,
            fx_presets: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPatch {
    pub output_dir: Option<String>,
    pub save_as: Option<SaveAs>,
    pub mp3_bitrate: Option<u32>,
    pub video_height: Option<u32>,
    pub video_container: Option<String>,
    pub video_keep_audio: Option<bool>,
    pub concurrency: Option<u8>,
    pub filename_template: Option<String>,
    pub embed_metadata: Option<bool>,
    pub collection_folders: Option<bool>,
    pub number_tracks: Option<bool>,
    pub skip_downloaded: Option<bool>,
    pub sponsorblock: Option<bool>,
    pub embed_subtitles: Option<bool>,
    pub subtitle_langs: Option<String>,
    pub embed_chapters: Option<bool>,
    pub cookies_browser: Option<String>,
    pub cookies_file: Option<String>,
    pub api_enabled: Option<bool>,
    pub check_updates: Option<bool>,
    pub artist_info: Option<bool>,
    pub api_port: Option<u16>,
    pub api_read: Option<bool>,
    pub api_control: Option<bool>,
    pub api_links: Option<bool>,
    pub proxy: Option<String>,
    pub rate_limit: Option<String>,
    pub ytdlp_path: Option<String>,
    pub ffmpeg_path: Option<String>,
    pub watch_clipboard: Option<bool>,
    pub theme: Option<String>,
    pub library_folders: Option<Vec<String>>,
    pub playlists_dir: Option<String>,
    pub playlist_copy: Option<bool>,
    pub lyrics_providers: Option<Vec<ProviderCfg>>,
    pub musixmatch_key: Option<String>,
    pub custom_lyrics: Option<Vec<CustomProvider>>,
    pub lyrics_on_download: Option<bool>,
    pub fx_presets: Option<Vec<serde_json::Value>>,
}

impl Settings {
    pub fn accept_from_installer(&mut self) -> bool {
        if self.rights_ok() || !cfg!(windows) {
            return false;
        }
        let marker = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("rights-accepted")));
        let ok = marker.and_then(|m| std::fs::read_to_string(m).ok()).is_some_and(|t| t.trim() == format!("terms={TERMS_VERSION}"));
        if ok {
            self.accept_rights();
        }
        ok
    }

    pub fn rights_ok(&self) -> bool {
        self.rights_version == TERMS_VERSION
    }

    pub fn accept_rights(&mut self) {
        self.rights_version = TERMS_VERSION;
        self.rights_accepted_at = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs());
    }

    pub fn revoke_rights(&mut self) {
        self.rights_version = 0;
        self.rights_accepted_at = None;
    }

    pub fn apply(&mut self, p: SettingsPatch) {
        macro_rules! set {
            ($($f:ident),*) => { $( if let Some(v) = p.$f { self.$f = v; } )* };
        }
        set!(save_as, embed_metadata, collection_folders, number_tracks, skip_downloaded, sponsorblock, embed_subtitles, embed_chapters, watch_clipboard);
        set!(api_enabled, api_read, api_control, api_links, check_updates, artist_info, video_keep_audio);
        if let Some(p) = p.api_port.filter(|p| *p >= 1024) {
            self.api_port = p;
        }
        set!(cookies_browser, cookies_file, proxy, rate_limit, ytdlp_path, ffmpeg_path, subtitle_langs, musixmatch_key, playlist_copy, lyrics_on_download, custom_lyrics, fx_presets);
        if let Some(v) = p.library_folders {
            let mut seen = std::collections::HashSet::new();
            self.library_folders = v.into_iter().map(|f| f.trim().to_string()).filter(|f| !f.is_empty() && seen.insert(f.clone())).collect();
        }
        if let Some(v) = p.playlists_dir.filter(|v| !v.trim().is_empty()) {
            self.playlists_dir = v;
        }
        if let Some(mut v) = p.lyrics_providers {
            for d in default_providers() {
                if !v.iter().any(|x| x.id == d.id) {
                    v.push(d);
                }
            }
            self.lyrics_providers = v;
        }
        if let Some(v) = p.output_dir.filter(|v| !v.trim().is_empty()) {
            self.output_dir = v;
        }
        if let Some(v) = p.mp3_bitrate {
            self.mp3_bitrate = snap_bitrate(v, None);
        }
        if let Some(v) = p.video_height {
            self.video_height = v;
        }
        if let Some(v) = p.video_container.filter(|v| matches!(v.as_str(), "mp4" | "mkv" | "webm")) {
            self.video_container = v;
        }
        if let Some(v) = p.concurrency {
            self.concurrency = v.clamp(1, 8);
        }
        if let Some(v) = p.filename_template {
            self.filename_template = if v.trim().is_empty() { Settings::default().filename_template } else { v };
        }
        if let Some(v) = p.theme.filter(|v| matches!(v.as_str(), "system" | "light" | "dark")) {
            self.theme = v;
        }
    }
}

pub fn path() -> PathBuf {
    match crate::engine::portable_root() {
        Some(root) => root.join("settings.json"),
        None => dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("atmosphere").join("settings.json"),
    }
}

pub fn load() -> Settings {
    let mut s: Settings = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for d in default_providers() {
        if !s.lyrics_providers.iter().any(|x| x.id == d.id) {
            s.lyrics_providers.push(d);
        }
    }
    if s.library_folders.is_empty() {
        s.library_folders.push(s.output_dir.clone());
    }
    s
}

pub fn save(s: &Settings) -> Result<()> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    std::fs::rename(&tmp, &p).context("Couldn't save settings")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_bitrates_to_source() {
        assert_eq!(snap_bitrate(320, Some(160)), 160);
        assert_eq!(snap_bitrate(320, Some(150)), 128);
        assert_eq!(snap_bitrate(192, None), 192);
        assert_eq!(snap_bitrate(320, Some(40)), 64);
    }

    #[test]
    fn applies_patches_safely() {
        let mut s = Settings::default();
        s.apply(SettingsPatch { concurrency: Some(40), theme: Some("neon".into()), mp3_bitrate: Some(200), ..Default::default() });
        assert_eq!(s.concurrency, 8);
        assert_eq!(s.theme, "system");
        assert_eq!(s.mp3_bitrate, 192);
    }
}

#[cfg(test)]
mod terms_tests {
    #[test]
    fn installer_records_the_current_terms_version() {
        let hooks = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/installer/hooks.nsh")).unwrap();
        assert!(hooks.contains(&format!("!define ATMO_TERMS_VERSION {}", super::TERMS_VERSION)), "bump ATMO_TERMS_VERSION in installer/hooks.nsh along with TERMS_VERSION");
    }
}
