// atmosphere · artistinfo.rs · Finds artist photos, bios and logos from keyless public sources: Deezer, Wikipedia and Wikidata
use anyhow::Result;
use base64::Engine as _;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;

const MAX_IMAGE: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub struct Found {
    pub photo: Option<Vec<u8>>,
    pub bio: Option<String>,
    pub bio_source: Option<String>,
    pub logo: Option<Vec<u8>>,
}

pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

static MUSIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(singer|rapper|musician|band|group|duo|trio|dj|disc jockey|producer|songwriter|composer|vocalist|guitarist|pianist|drummer|music|hip[- ]hop|rock|pop|electronic|orchestra|ensemble|artist)\b").unwrap()
});

pub fn is_music_description(d: &str) -> bool {
    MUSIC.is_match(d) && !Regex::new(r"(?i)\b(album|song|single|ep|discography|film|episode|video game)\b").unwrap().is_match(d.split(" by ").next().unwrap_or(d))
}

fn title_matches(key: &str, name: &str) -> bool {
    let title = key.replace('_', " ");
    let base = title.split(" (").next().unwrap_or(&title);
    norm(base) == norm(name)
}

async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let r = http.get(url).timeout(Duration::from_secs(15)).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json().await.ok()
}

async fn get_bytes(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let r = http.get(url).timeout(Duration::from_secs(20)).send().await.ok()?;
    if !r.status().is_success() || r.content_length().is_some_and(|n| n as usize > MAX_IMAGE) {
        return None;
    }
    let b = r.bytes().await.ok()?;
    (b.len() <= MAX_IMAGE && !b.is_empty()).then(|| b.to_vec())
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn path_enc(s: &str) -> String {
    s.replace(' ', "_")
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn deezer_photo(http: &reqwest::Client, name: &str) -> Option<Vec<u8>> {
    let v = get_json(http, &format!("https://api.deezer.com/search/artist?q={}&limit=8", enc(name))).await?;
    let want = norm(name);
    let best = v["data"]
        .as_array()?
        .iter()
        .filter(|a| a["name"].as_str().is_some_and(|n| norm(n) == want))
        .filter(|a| a["picture_xl"].as_str().is_some_and(|u| !u.contains("/artist//")))
        .max_by_key(|a| a["nb_fan"].as_u64().unwrap_or(0))?;
    get_bytes(http, best["picture_xl"].as_str()?).await
}

async fn wikipedia(http: &reqwest::Client, name: &str) -> Option<(String, String, Option<String>)> {
    let v = get_json(http, &format!("https://en.wikipedia.org/w/rest.php/v1/search/page?q={}&limit=10", enc(name))).await?;
    let pages = v["pages"].as_array()?;
    let pick = pages
        .iter()
        .filter(|p| p["key"].as_str().is_some_and(|k| title_matches(k, name)))
        .find(|p| p["description"].as_str().is_some_and(is_music_description))?;
    let key = pick["key"].as_str()?;
    let s = get_json(http, &format!("https://en.wikipedia.org/api/rest_v1/page/summary/{}", path_enc(key))).await?;
    if s["type"].as_str() != Some("standard") {
        return None;
    }
    let extract = s["extract"].as_str()?.trim().to_string();
    if extract.len() < 40 {
        return None;
    }
    let url = s["content_urls"]["desktop"]["page"].as_str().unwrap_or_default().to_string();
    Some((extract, url, s["wikibase_item"].as_str().map(String::from)))
}

async fn wikidata_file(http: &reqwest::Client, qid: &str, prop: &str) -> Option<Vec<u8>> {
    if !qid.starts_with('Q') || !qid[1..].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v = get_json(http, &format!("https://www.wikidata.org/wiki/Special:EntityData/{qid}.json")).await?;
    let file = v["entities"][qid]["claims"][prop][0]["mainsnak"]["datavalue"]["value"].as_str()?;
    get_bytes(http, &format!("https://commons.wikimedia.org/wiki/Special:FilePath/{}?width=800", path_enc(file))).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoChoice {
    pub name: String,
    pub fans: u64,
    pub thumb: String,
    pub full: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageChoice {
    pub key: String,
    pub title: String,
    pub description: String,
    pub music: bool,
}

pub async fn choices(http: &reqwest::Client, query: &str) -> (Vec<PhotoChoice>, Vec<PageChoice>) {
    let q = enc(query);
    let photos = async {
        let Some(v) = get_json(http, &format!("https://api.deezer.com/search/artist?q={q}&limit=12")).await else { return Vec::new() };
        v["data"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|a| {
                        let full = a["picture_xl"].as_str().filter(|u| !u.contains("/artist//"))?;
                        Some(PhotoChoice { name: a["name"].as_str()?.to_string(), fans: a["nb_fan"].as_u64().unwrap_or(0), thumb: a["picture_medium"].as_str().unwrap_or(full).to_string(), full: full.to_string() })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let pages = async {
        let Some(v) = get_json(http, &format!("https://en.wikipedia.org/w/rest.php/v1/search/page?q={q}&limit=8")).await else { return Vec::new() };
        v["pages"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|p| {
                        let description = p["description"].as_str().unwrap_or_default().to_string();
                        Some(PageChoice { key: p["key"].as_str()?.to_string(), title: p["title"].as_str()?.to_string(), music: is_music_description(&description), description })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (mut photos, pages) = tokio::join!(photos, pages);
    let thumbs = futures_util::future::join_all(photos.iter().map(|p| get_bytes(http, &p.thumb))).await;
    for (p, bytes) in photos.iter_mut().zip(thumbs) {
        p.thumb = bytes.map(|b| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b))).unwrap_or_default();
    }
    photos.retain(|p| !p.thumb.is_empty());
    (photos, pages)
}

pub fn allowed_photo(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some_and(|h| h.ends_with(".dzcdn.net")))
}

pub async fn chosen(http: &reqwest::Client, photo: Option<&str>, page: Option<&str>) -> Found {
    let mut found = Found::default();
    if let Some(url) = photo.filter(|u| allowed_photo(u)) {
        found.photo = get_bytes(http, url).await;
    }
    if let Some(key) = page.filter(|k| !k.is_empty() && k.len() < 300 && !k.contains('/')) {
        if let Some(s) = get_json(http, &format!("https://en.wikipedia.org/api/rest_v1/page/summary/{}", path_enc(key))).await {
            if s["type"].as_str() == Some("standard") {
                found.bio = s["extract"].as_str().map(|t| t.trim().to_string());
                found.bio_source = s["content_urls"]["desktop"]["page"].as_str().map(String::from);
                if let Some(q) = s["wikibase_item"].as_str() {
                    found.logo = wikidata_file(http, q, "P154").await;
                    if found.photo.is_none() && photo.is_none() {
                        found.photo = wikidata_file(http, q, "P18").await;
                    }
                }
            }
        }
    }
    found
}

pub async fn find(http: &reqwest::Client, name: &str, want_photo: bool, want_bio: bool, want_logo: bool) -> Result<Found> {
    let mut found = Found::default();
    let photo = async {
        if want_photo {
            deezer_photo(http, name).await
        } else {
            None
        }
    };
    let wiki = async {
        if want_bio || want_logo || want_photo {
            wikipedia(http, name).await
        } else {
            None
        }
    };
    let (photo, wiki) = tokio::join!(photo, wiki);
    found.photo = photo;
    if let Some((bio, url, qid)) = wiki {
        if want_bio {
            found.bio = Some(bio);
            found.bio_source = Some(url);
        }
        if let Some(q) = qid {
            if want_logo {
                found.logo = wikidata_file(http, &q, "P154").await;
            }
            if want_photo && found.photo.is_none() {
                found.photo = wikidata_file(http, &q, "P18").await;
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_names_and_music_pages_only() {
        assert!(title_matches("Daft_Punk", "Daft Punk"));
        assert!(title_matches("Nirvana_(band)", "Nirvana"));
        assert!(!title_matches("Discovery_(Daft_Punk_album)", "Daft Punk"));
        assert!(is_music_description("French electronic music duo (1993–2021)"));
        assert!(is_music_description("American rapper and singer"));
        assert!(!is_music_description("2001 studio album by Daft Punk"));
        assert!(!is_music_description("City in France"));
        assert_eq!(norm("Kühvt!"), "kühvt");
        assert_eq!(path_enc("Daft Punk logo.svg"), "Daft_Punk_logo.svg");
        assert_eq!(path_enc("Beyoncé"), "Beyonc%C3%A9");
        assert!(allowed_photo("https://cdn-images.dzcdn.net/images/artist/abc/1000x1000.jpg"));
        assert!(!allowed_photo("http://cdn-images.dzcdn.net/x.jpg"));
        assert!(!allowed_photo("https://evil.example/dzcdn.net.jpg"));
        assert!(!allowed_photo("file:///C:/Windows/win.ini"));
    }

    #[tokio::test]
    #[ignore]
    async fn live_choices_include_photos() {
        let http = reqwest::Client::builder().user_agent(concat!("atmosphere/", env!("CARGO_PKG_VERSION"))).build().unwrap();
        for q in ["Daft Punk", "Drake", "Taylor Swift", "Westwood"] {
            let (photos, pages) = choices(&http, q).await;
            eprintln!("{q}: {} photos, {} pages", photos.len(), pages.len());
            assert!(!photos.is_empty(), "no photos for {q}");
            assert!(photos.iter().all(|p| p.thumb.starts_with("data:image/jpeg;base64,") && p.full.starts_with("https://")));
        }
    }

    #[tokio::test]
    #[ignore]
    async fn live_finds_daft_punk() {
        let http = reqwest::Client::builder().user_agent("Atmosphere/test (desktop music player)").build().unwrap();
        let f = find(&http, "Daft Punk", true, true, true).await.unwrap();
        assert!(f.photo.is_some_and(|p| p.len() > 10_000));
        assert!(f.bio.is_some_and(|b| b.contains("Daft Punk")));
        assert!(f.bio_source.is_some_and(|u| u.contains("wikipedia.org")));
        assert!(f.logo.is_some());
    }
}
