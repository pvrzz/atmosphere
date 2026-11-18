// atmosphere · history.rs · Persists the list of finished downloads so the Library view survives restarts.
use crate::downloader::JobRequest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: usize = 5000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub platform: String,
    pub url: String,
    pub path: String,
    pub format: Option<String>,
    pub collection: Option<String>,
    pub thumbnail: Option<String>,
    pub at: u64,
}

impl Record {
    pub fn new(req: &JobRequest, path: &Path, format: Option<String>) -> Self {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let pick = |edited: &str, original: &str| if edited.trim().is_empty() { original.to_string() } else { edited.to_string() };
        Self {
            id: format!("{at}-{}", req.entry.index),
            title: pick(&req.meta.title, &req.entry.title),
            artist: pick(&req.meta.artist, &req.entry.artist),
            platform: req.entry.platform.clone(),
            url: req.entry.url.clone(),
            path: path.to_string_lossy().into_owned(),
            format,
            collection: req.folder.clone(),
            thumbnail: req.entry.thumbnail.clone(),
            at,
        }
    }
}

pub struct History {
    file: PathBuf,
    items: Mutex<Vec<Record>>,
}

impl History {
    pub fn load(file: PathBuf) -> Self {
        let items = std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { file, items: Mutex::new(items) }
    }

    fn persist(&self, items: &[Record]) {
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(data) = serde_json::to_vec(items) {
            let tmp = self.file.with_extension("json.tmp");
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(&tmp, &self.file);
            }
        }
    }

    pub fn add(&self, record: Record) {
        let mut items = self.items.lock().unwrap();
        items.retain(|r| r.path != record.path);
        items.insert(0, record);
        items.truncate(LIMIT);
        self.persist(&items);
    }

    pub fn list(&self) -> Vec<Record> {
        self.items.lock().unwrap().iter().map(|r| {
            let mut r = r.clone();
            if !Path::new(&r.path).exists() {
                r.path = String::new();
            }
            r
        }).collect()
    }

    pub fn remove(&self, ids: &[String]) {
        let mut items = self.items.lock().unwrap();
        items.retain(|r| !ids.contains(&r.id));
        self.persist(&items);
    }

    pub fn clear(&self) {
        let mut items = self.items.lock().unwrap();
        items.clear();
        self.persist(&items);
    }
}
