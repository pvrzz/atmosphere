// atmosphere · backup.rs · Saves and restores settings, library, stats, liked songs and artist photos as one .zip
use crate::engine::data_dir;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::Path;

const DATA_FILES: &[&str] = &["library.json", "stats.json", "artists.json", "hidden.json", "history.json", "archive.txt", "linked.json"];
const MARKER: &str = "atmosphere-backup.json";

fn artist_photos(data: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(data.join("artists.json")) else { return Vec::new() };
    let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    v.as_object()
        .map(|m| m.values().filter_map(|a| a["image"].as_str().map(|s| format!("{s}.jpg"))).collect())
        .unwrap_or_default()
}

fn safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains("..") && !name.contains('\\') && !name.starts_with('/') && !name.contains(':')
}

pub fn create(dest: &Path) -> Result<usize> {
    create_from(&data_dir(), &crate::settings::path(), dest)
}

pub fn restore(src: &Path) -> Result<usize> {
    restore_into(&data_dir(), &crate::settings::path(), src)
}

fn create_from(data: &Path, settings: &Path, dest: &Path) -> Result<usize> {
    let tmp = dest.with_extension("zip.part");
    let file = std::fs::File::create(&tmp).with_context(|| format!("Couldn't write to {}", dest.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let put = |zip: &mut zip::ZipWriter<std::fs::File>, name: &str, data: &[u8]| -> Result<()> {
        zip.start_file(name, opts)?;
        zip.write_all(data)?;
        Ok(())
    };
    let mut count = 0;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    put(&mut zip, MARKER, serde_json::json!({ "app": "atmosphere", "version": env!("CARGO_PKG_VERSION"), "created": stamp }).to_string().as_bytes())?;
    if let Ok(bytes) = std::fs::read(settings) {
        put(&mut zip, "settings.json", &bytes)?;
        count += 1;
    }
    for name in DATA_FILES {
        if let Ok(bytes) = std::fs::read(data.join(name)) {
            put(&mut zip, name, &bytes)?;
            count += 1;
        }
    }
    for photo in artist_photos(data) {
        if let Ok(bytes) = std::fs::read(data.join("art").join(&photo)) {
            put(&mut zip, &format!("art/{photo}"), &bytes)?;
            count += 1;
        }
    }
    zip.finish()?;
    std::fs::rename(&tmp, dest).or_else(|_| std::fs::copy(&tmp, dest).map(|_| ()).and_then(|_| std::fs::remove_file(&tmp)))?;
    Ok(count)
}

fn restore_into(data: &Path, settings: &Path, src: &Path) -> Result<usize> {
    let file = std::fs::File::open(src).context("Couldn't open that backup")?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| anyhow!("That file isn't an Atmosphere backup."))?;
    if zip.by_name(MARKER).is_err() {
        bail!("That file isn't an Atmosphere backup.");
    }
    let mut count = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().map(|n| n.to_string()).unwrap_or_default();
        if !entry.is_file() || !safe_name(&name) || name == MARKER {
            continue;
        }
        let target = if name == "settings.json" {
            settings.to_path_buf()
        } else if DATA_FILES.contains(&name.as_str()) {
            data.join(&name)
        } else if let Some(photo) = name.strip_prefix("art/").filter(|p: &&str| !p.contains('/') && p.ends_with(".jpg")) {
            data.join("art").join(photo)
        } else {
            continue;
        };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&target, bytes)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_round_trips_and_rejects_strangers() {
        let root = std::env::temp_dir().join(format!("atmo-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("art")).unwrap();
        std::fs::write(root.join("stats.json"), br#"{"liked":["a"]}"#).unwrap();
        std::fs::write(root.join("artists.json"), br#"{"x":{"name":"X","image":"p1"}}"#).unwrap();
        std::fs::write(root.join("art").join("p1.jpg"), b"jpg").unwrap();
        std::fs::write(root.join("art").join("cover.jpg"), b"thumb").unwrap();
        let zip_path = root.join("b.zip");
        let set = root.join("cfg").join("settings.json");
        std::fs::create_dir_all(set.parent().unwrap()).unwrap();
        std::fs::write(&set, b"{}").unwrap();
        assert!(create_from(&root, &set, &zip_path).unwrap() >= 4);
        std::fs::remove_file(&set).unwrap();
        std::fs::remove_file(root.join("stats.json")).unwrap();
        std::fs::remove_file(root.join("art").join("p1.jpg")).unwrap();
        assert!(restore_into(&root, &set, &zip_path).unwrap() >= 4);
        assert!(set.exists());
        assert_eq!(std::fs::read(root.join("stats.json")).unwrap(), br#"{"liked":["a"]}"#);
        assert!(root.join("art").join("p1.jpg").exists());
        std::fs::write(root.join("not.zip"), b"nope").unwrap();
        assert!(restore_into(&root, &set, &root.join("not.zip")).is_err());
        assert!(!safe_name("../x") && !safe_name("C:x") && safe_name("art/p.jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
