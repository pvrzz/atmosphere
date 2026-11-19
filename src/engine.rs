// atmosphere · engine.rs · Locates, installs and runs the yt-dlp and ffmpeg tools that power every download.
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

#[cfg(windows)]
const EXE: &str = ".exe";
#[cfg(not(windows))]
const EXE: &str = "";

const YTDLP_RELEASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
const FFMPEG_RELEASE: &str = "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest";

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub path: Option<String>,
    pub version: Option<String>,
    pub managed: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub ytdlp: ToolInfo,
    pub ffmpeg: ToolInfo,
    pub can_install_ffmpeg: bool,
}

pub fn bin_dir() -> PathBuf {
    data_dir().join("bin")
}

pub fn portable_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    ["portable", "portable.txt", ".portable"].iter().any(|m| dir.join(m).is_file()).then(|| dir.join("AtmosphereData"))
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ATMOSPHERE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    portable_root().unwrap_or_else(|| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("atmosphere"))
}

pub fn temp_root() -> PathBuf {
    data_dir().join("temp")
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(unix) {
        let home = dirs::home_dir().unwrap_or_default();
        for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/snap/bin", "/opt/local/bin"] {
            dirs.push(PathBuf::from(extra));
        }
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
    }
    dirs
}

fn on_path(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{EXE}");
    search_dirs().into_iter().map(|d| d.join(&file)).find(|c| c.is_file())
}

fn resolve(custom: &str, name: &str) -> (Option<PathBuf>, bool) {
    if !custom.trim().is_empty() {
        let p = PathBuf::from(custom.trim());
        return (p.is_file().then_some(p), false);
    }
    let managed = bin_dir().join(format!("{name}{EXE}"));
    if managed.is_file() {
        return (Some(managed), true);
    }
    (on_path(name), false)
}

pub fn find_tool(name: &str) -> Option<PathBuf> {
    resolve("", name).0
}

pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd
}

async fn version_of(path: &Path, args: &[&str]) -> Option<String> {
    let out = command(path).args(args).stdout(Stdio::piped()).stderr(Stdio::null()).output().await.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?.trim();
    let v = line.strip_prefix("ffmpeg version ").map(|r| r.split_whitespace().next().unwrap_or(r)).unwrap_or(line);
    (!v.is_empty()).then(|| v.to_string())
}

pub struct Tools {
    pub ytdlp: PathBuf,
    pub ffmpeg: Option<PathBuf>,
}

pub fn locate(ytdlp_custom: &str, ffmpeg_custom: &str) -> Result<Tools> {
    let (ytdlp, _) = resolve(ytdlp_custom, "yt-dlp");
    let ytdlp = ytdlp.ok_or_else(|| anyhow!("yt-dlp isn't set up yet. Open Settings → Engine to install it."))?;
    let (ffmpeg, _) = resolve(ffmpeg_custom, "ffmpeg");
    Ok(Tools { ytdlp, ffmpeg })
}

pub async fn status(ytdlp_custom: &str, ffmpeg_custom: &str) -> ToolStatus {
    let (yp, ym) = resolve(ytdlp_custom, "yt-dlp");
    let (fp, fm) = resolve(ffmpeg_custom, "ffmpeg");
    let yv = match &yp {
        Some(p) => version_of(p, &["--version"]).await,
        None => None,
    };
    let fv = match &fp {
        Some(p) => version_of(p, &["-version"]).await,
        None => None,
    };
    ToolStatus {
        ytdlp: ToolInfo { path: yp.map(|p| p.to_string_lossy().into_owned()), version: yv, managed: ym },
        ffmpeg: ToolInfo { path: fp.map(|p| p.to_string_lossy().into_owned()), version: fv, managed: fm },
        can_install_ffmpeg: ffmpeg_asset().is_some(),
    }
}

fn ytdlp_asset() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => "yt-dlp.exe",
        ("macos", _) => "yt-dlp_macos",
        ("linux", "aarch64") => "yt-dlp_linux_aarch64",
        _ => "yt-dlp_linux",
    }
}

fn ffmpeg_asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => Some("ffmpeg-master-latest-winarm64-gpl.zip"),
        ("windows", _) => Some("ffmpeg-master-latest-win64-gpl.zip"),
        ("linux", "aarch64") => Some("ffmpeg-master-latest-linuxarm64-gpl.tar.xz"),
        ("linux", "x86_64") => Some("ffmpeg-master-latest-linux64-gpl.tar.xz"),
        _ => None,
    }
}

fn wanted(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let file = path.rsplit('/').next().unwrap_or_default();
    (path.contains("/bin/") && (file == format!("ffmpeg{EXE}") || file == format!("ffprobe{EXE}"))).then(|| file.to_string())
}

fn extract_ffmpeg(asset: &str, data: Vec<u8>, dir: &Path) -> Result<PathBuf> {
    let mut found = None;
    let mut keep = |file: String, buf: Vec<u8>| -> Result<()> {
        let dest = dir.join(&file);
        write_exec(&dest, &buf)?;
        if file.starts_with("ffmpeg") {
            found = Some(dest);
        }
        Ok(())
    };
    if asset.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            if let Some(file) = wanted(&entry.name()?) {
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut buf)?;
                keep(file, buf)?;
            }
        }
    } else {
        let mut tar_bytes = Vec::new();
        lzma_rs::xz_decompress(&mut std::io::BufReader::new(std::io::Cursor::new(data)), &mut tar_bytes)
            .map_err(|e| anyhow!("Couldn't unpack ffmpeg ({e})"))?;
        let mut archive = tar::Archive::new(std::io::Cursor::new(tar_bytes));
        for entry in archive.entries()? {
            let mut entry = entry?;
            let name = entry.path()?.to_string_lossy().into_owned();
            if let Some(file) = wanted(&name) {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                keep(file, buf)?;
            }
        }
    }
    found.ok_or_else(|| anyhow!("ffmpeg wasn't in the downloaded archive"))
}

async fn fetch(http: &reqwest::Client, url: &str, mut progress: impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>> {
    use futures_util::StreamExt;
    let resp = http.get(url).send().await?.error_for_status().with_context(|| format!("Download failed: {url}"))?;
    let total = resp.content_length();
    let mut body = resp.bytes_stream();
    let mut data = Vec::with_capacity(total.unwrap_or(0) as usize);
    while let Some(chunk) = body.next().await {
        data.extend_from_slice(&chunk?);
        progress(data.len() as u64, total);
    }
    Ok(data)
}

fn expected_hash(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset).then(|| hash.to_ascii_lowercase())
    })
}

fn verify(data: &[u8], sums: &str, asset: &str) -> Result<()> {
    let expected = expected_hash(sums, asset).ok_or_else(|| anyhow!("No published checksum for {asset}"))?;
    let actual = Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect::<String>();
    if actual != expected {
        bail!("Checksum mismatch for {asset}. The download was discarded.");
    }
    Ok(())
}

fn write_exec(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("download");
    std::fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub async fn install_ytdlp(http: &reqwest::Client, progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let asset = ytdlp_asset();
    let sums = http.get(format!("{YTDLP_RELEASE}/SHA2-256SUMS")).send().await?.error_for_status()?.text().await?;
    let data = fetch(http, &format!("{YTDLP_RELEASE}/{asset}"), progress).await?;
    verify(&data, &sums, asset)?;
    let dest = bin_dir().join(format!("yt-dlp{EXE}"));
    write_exec(&dest, &data)?;
    Ok(dest)
}

pub async fn install_ffmpeg(http: &reqwest::Client, progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let Some(asset) = ffmpeg_asset() else {
        bail!(if cfg!(target_os = "macos") {
            "On macOS, install ffmpeg with Homebrew (`brew install ffmpeg`). Atmosphere finds it automatically."
        } else {
            "Install ffmpeg with your package manager, then restart Atmosphere."
        });
    };
    let sums = http.get(format!("{FFMPEG_RELEASE}/checksums.sha256")).send().await?.error_for_status()?.text().await?;
    let data = fetch(http, &format!("{FFMPEG_RELEASE}/{asset}"), progress).await?;
    verify(&data, &sums, asset)?;
    let dir = bin_dir();
    tokio::task::spawn_blocking(move || extract_ffmpeg(asset, data, &dir)).await?
}

pub async fn update_ytdlp(path: &Path) -> Result<String> {
    let out = command(path).arg("-U").stdout(Stdio::piped()).stderr(Stdio::piped()).output().await?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let line = text
        .lines()
        .rev()
        .find(|l| l.contains("Updated") || l.contains("up to date") || l.contains("ERROR"))
        .unwrap_or("Update finished")
        .trim()
        .trim_start_matches("ERROR: ")
        .to_string();
    if !out.status.success() {
        bail!(line);
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_checksums() {
        let sums = "abc123  yt-dlp.exe\nDEF456 *yt-dlp_linux\n";
        assert_eq!(expected_hash(sums, "yt-dlp.exe").as_deref(), Some("abc123"));
        assert_eq!(expected_hash(sums, "yt-dlp_linux").as_deref(), Some("def456"));
        assert!(expected_hash(sums, "nope").is_none());
        let data = b"hello";
        let good = format!("{}  file.bin", Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect::<String>());
        assert!(verify(data, &good, "file.bin").is_ok());
        assert!(verify(b"tampered", &good, "file.bin").is_err());
    }

    #[test]
    fn picks_ffmpeg_binaries_from_archives() {
        let exe = |n: &str| format!("{n}{EXE}");
        assert_eq!(wanted(&format!("ffmpeg-master-latest/bin/{}", exe("ffmpeg"))), Some(exe("ffmpeg")));
        assert_eq!(wanted(&format!("ffmpeg-master-latest\\bin\\{}", exe("ffprobe"))), Some(exe("ffprobe")));
        assert_eq!(wanted("ffmpeg-master-latest/doc/ffmpeg.html"), None);
        assert_eq!(wanted(&format!("ffmpeg-master-latest/bin/{}", exe("ffplay"))), None);
    }
}
