// atmosphere · albuminfo.rs · Finds album covers, descriptions and logos from keyless public sources: Deezer, Apple, TheAudioDB, the Cover Art Archive, Wikipedia, Wikidata and Wikimedia Commons
use crate::artistinfo::{audiodb_logo, commons_file, enc, get_bytes, get_json, norm, path_enc, wikidata_file, wikidata_file_name, PageChoice};
use anyhow::{anyhow, bail, Result};
use base64::Engine as _;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

const AUDIODB: &str = "https://www.theaudiodb.com/api/v1/json/123";
pub const COVER_EDGE: u32 = 1500;

#[derive(Default)]
pub struct Found {
    pub bio: Option<String>,
    pub bio_source: Option<String>,
    pub logo: Option<Vec<u8>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverChoice {
    pub source: String,
    pub title: String,
    pub artist: String,
    pub thumb: String,
    pub full: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogoChoice {
    pub source: String,
    pub name: String,
    pub thumb: String,
    pub full: String,
}

static ALBUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(album|ep|extended play|mixtape|soundtrack|compilation|live recording)\b").unwrap());

fn base(s: &str) -> String {
    norm(s.split(" (").next().unwrap_or(s).split(" [").next().unwrap_or(s))
}

pub fn is_album_description(d: &str, artist: &str) -> bool {
    let a = norm(artist);
    ALBUM.is_match(d) && (a.is_empty() || a == "variousartists" || norm(d).contains(&a))
}

async fn summary(http: &reqwest::Client, key: &str) -> Option<(String, String, Option<String>)> {
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

async fn wikipedia(http: &reqwest::Client, artist: &str, title: &str) -> Option<(String, String, Option<String>)> {
    let v = get_json(http, &format!("https://en.wikipedia.org/w/rest.php/v1/search/page?q={}&limit=10", enc(&format!("{title} {artist} album")))).await?;
    let want = base(title);
    let key = v["pages"]
        .as_array()?
        .iter()
        .filter(|p| p["key"].as_str().is_some_and(|k| base(&k.replace('_', " ")) == want))
        .find(|p| p["description"].as_str().is_some_and(|d| is_album_description(d, artist)))?["key"]
        .as_str()?
        .to_string();
    summary(http, &key).await
}

async fn audiodb_album(http: &reqwest::Client, artist: &str, title: &str) -> Option<Value> {
    let v = get_json(http, &format!("{AUDIODB}/searchalbum.php?s={}&a={}", enc(artist), enc(title))).await?;
    let want = base(title);
    v["album"].as_array()?.iter().find(|a| a["strAlbum"].as_str().is_some_and(|n| base(n) == want)).cloned()
}

fn audiodb_bio(a: &Value) -> Option<(String, String)> {
    let text = a["strDescriptionEN"].as_str().or(a["strDescription"].as_str())?.trim();
    (text.len() >= 40).then(|| (text.to_string(), format!("https://www.theaudiodb.com/album/{}", a["idAlbum"].as_str().unwrap_or_default())))
}

pub async fn find(http: &reqwest::Client, artist: &str, title: &str, want_bio: bool, want_logo: bool) -> Found {
    let mut found = Found::default();
    if !want_bio && !want_logo {
        return found;
    }
    let (wiki, adb) = tokio::join!(wikipedia(http, artist, title), async { if want_bio { audiodb_album(http, artist, title).await } else { None } });
    if let Some((bio, url, qid)) = wiki {
        if want_bio {
            found.bio = Some(bio);
            found.bio_source = Some(url);
        }
        if let (true, Some(q)) = (want_logo, qid) {
            found.logo = wikidata_file(http, &q, "P154").await;
        }
    }
    if want_bio && found.bio.is_none() {
        if let Some((bio, url)) = adb.as_ref().and_then(audiodb_bio) {
            found.bio = Some(bio);
            found.bio_source = Some(url);
        }
    }
    found
}

pub async fn chosen(http: &reqwest::Client, key: &str) -> Found {
    let mut found = Found::default();
    if key.is_empty() || key.len() >= 300 || key.contains('/') {
        return found;
    }
    if let Some((bio, url, qid)) = summary(http, key).await {
        found.bio = Some(bio);
        found.bio_source = Some(url);
        if let Some(q) = qid {
            found.logo = wikidata_file(http, &q, "P154").await;
        }
    }
    found
}

pub fn allowed_cover(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some_and(|h| h.ends_with(".dzcdn.net") || h.ends_with(".mzstatic.com") || h == "theaudiodb.com" || h.ends_with(".theaudiodb.com") || h == "coverartarchive.org")
    })
}

pub async fn choices(http: &reqwest::Client, query: &str, artist: &str, title: &str) -> (Vec<CoverChoice>, Vec<PageChoice>) {
    let q = enc(query);
    let deezer = async {
        let Some(v) = get_json(http, &format!("https://api.deezer.com/search/album?q={q}&limit=10")).await else { return Vec::new() };
        v["data"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|a| {
                        let full = a["cover_xl"].as_str().filter(|u| !u.contains("/cover//"))?;
                        Some(CoverChoice { source: "Deezer".into(), title: a["title"].as_str()?.into(), artist: a["artist"]["name"].as_str().unwrap_or_default().into(), thumb: a["cover_medium"].as_str().unwrap_or(full).into(), full: full.into() })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let apple = async {
        let Some(v) = get_json(http, &format!("https://itunes.apple.com/search?term={q}&entity=album&limit=10")).await else { return Vec::new() };
        v["results"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|a| {
                        let art = a["artworkUrl100"].as_str().filter(|u| u.contains("100x100bb"))?;
                        Some(CoverChoice { source: "Apple Music".into(), title: a["collectionName"].as_str()?.into(), artist: a["artistName"].as_str().unwrap_or_default().into(), thumb: art.replace("100x100bb", "300x300bb"), full: art.replace("100x100bb", &format!("{COVER_EDGE}x{COVER_EDGE}bb")) })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let audiodb = async {
        let Some(a) = audiodb_album(http, artist, title).await else { return Vec::new() };
        let name = a["strAlbum"].as_str().unwrap_or(title).to_string();
        let by = a["strArtist"].as_str().unwrap_or(artist).to_string();
        let mut out = Vec::new();
        if let Some(u) = a["strAlbumThumb"].as_str().filter(|u| allowed_cover(u)) {
            out.push(CoverChoice { source: "TheAudioDB".into(), title: name.clone(), artist: by.clone(), thumb: format!("{u}/preview"), full: u.into() });
        }
        if let Some(mb) = a["strMusicBrainzID"].as_str().filter(|m| m.len() == 36 && m.chars().all(|c| c.is_ascii_hexdigit() || c == '-')) {
            let root = format!("https://coverartarchive.org/release-group/{mb}");
            out.push(CoverChoice { source: "Cover Art Archive".into(), title: name, artist: by, thumb: format!("{root}/front-250"), full: format!("{root}/front-1200") });
        }
        out
    };
    let pages = async {
        let Some(v) = get_json(http, &format!("https://en.wikipedia.org/w/rest.php/v1/search/page?q={}&limit=8", enc(&format!("{query} album")))).await else { return Vec::new() };
        v["pages"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|p| {
                        let description = p["description"].as_str().unwrap_or_default().to_string();
                        Some(PageChoice { key: p["key"].as_str()?.into(), title: p["title"].as_str()?.into(), music: is_album_description(&description, artist), description })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let (dz, ap, adb, pages) = tokio::join!(deezer, apple, audiodb, pages);
    let mut covers: Vec<CoverChoice> = adb.into_iter().chain(dz).chain(ap).collect();
    let thumbs = futures_util::future::join_all(covers.iter().map(|c| get_bytes(http, &c.thumb))).await;
    for (c, bytes) in covers.iter_mut().zip(thumbs) {
        c.thumb = bytes.map(|b| format!("data:{};base64,{}", sniff(&b).unwrap_or("image/jpeg"), base64::engine::general_purpose::STANDARD.encode(b))).unwrap_or_default();
    }
    covers.retain(|c| !c.thumb.is_empty());
    (covers, pages)
}

pub fn allowed_logo(url: &str) -> bool {
    crate::artistinfo::allowed_logo(url) || url::Url::parse(url).is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("commons.wikimedia.org") && u.path().starts_with("/wiki/Special:FilePath/"))
}

fn looks_like_logo(stem: &str) -> bool {
    let s = stem.to_lowercase();
    s.contains("logo") || s.contains("wordmark") || s.contains("title")
}

fn names_album(stem: &str, want: &str, artist: &str) -> bool {
    let n = norm(stem);
    if !n.contains(want) {
        return false;
    }
    if !artist.is_empty() && n.contains(artist) {
        return true;
    }
    let rest: Vec<&str> = stem.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()) && !matches!(w.to_lowercase().as_str(), "logo" | "wordmark" | "title" | "album" | "text" | "cover")).collect();
    norm(&rest.join(" ")) == want
}

async fn commons_logos(http: &reqwest::Client, search: &str, want: &str, artist: &str) -> Vec<String> {
    let url = format!("https://commons.wikimedia.org/w/api.php?action=query&format=json&generator=search&gsrnamespace=6&gsrlimit=20&gsrsearch={}&prop=imageinfo&iiprop=mime", enc(search));
    let Some(v) = get_json(http, &url).await else { return Vec::new() };
    let Some(pages) = v["query"]["pages"].as_object() else { return Vec::new() };
    let mut hits: Vec<(bool, i64, String)> = pages
        .values()
        .filter_map(|p| {
            let mime = p["imageinfo"][0]["mime"].as_str()?;
            if mime != "image/png" && mime != "image/svg+xml" {
                return None;
            }
            let file = p["title"].as_str()?.strip_prefix("File:")?.to_string();
            let stem = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s);
            let (hit, rank) = (names_album(stem, want, artist), !looks_like_logo(stem));
            hit.then(|| (rank, p["index"].as_i64().unwrap_or(99), file))
        })
        .collect();
    hits.sort();
    hits.into_iter().map(|(_, _, f)| f).collect()
}

pub async fn logo_choices(http: &reqwest::Client, query: &str, artist: &str, title: &str) -> Vec<LogoChoice> {
    let (want, by) = (base(title), norm(artist));
    if want.is_empty() {
        return Vec::new();
    }
    let wikidata = async {
        let (_, _, qid) = wikipedia(http, artist, title).await?;
        wikidata_file_name(http, &qid?, "P154").await
    };
    let (by_query, by_title) = (format!("{query} logo"), format!("{title} logo"));
    let (wd, by_query, by_title, artist_logo) = tokio::join!(wikidata, commons_logos(http, &by_query, &want, &by), commons_logos(http, &by_title, &want, &by), audiodb_logo(http, artist));
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<LogoChoice> = wd
        .map(|f| ("Wikidata", f))
        .into_iter()
        .chain(by_query.into_iter().chain(by_title).map(|f| ("Wikimedia Commons", f)))
        .filter(|(_, f)| seen.insert(f.clone()))
        .take(12)
        .map(|(source, f)| LogoChoice { source: source.into(), name: f.rsplit_once('.').map_or(f.as_str(), |(s, _)| s).replace('_', " "), thumb: commons_file(&f, 320), full: commons_file(&f, 800) })
        .collect();
    if let Some(u) = artist_logo {
        out.push(LogoChoice { source: "TheAudioDB".into(), name: format!("{artist} (artist logo)"), thumb: format!("{u}/preview"), full: u });
    }
    let thumbs = futures_util::future::join_all(out.iter().map(|l| async { match get_bytes(http, &l.thumb).await { Some(b) if sniff(&b).is_some() => Some(b), _ => get_bytes(http, &l.full).await } })).await;
    for (l, bytes) in out.iter_mut().zip(thumbs) {
        l.thumb = bytes.filter(|b| sniff(b).is_some()).map(|b| format!("data:{};base64,{}", sniff(&b).unwrap_or("image/png"), base64::engine::general_purpose::STANDARD.encode(b))).unwrap_or_default();
    }
    out.retain(|l| !l.thumb.is_empty());
    out
}

pub async fn logo(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    if !allowed_logo(url) {
        bail!("That image isn't from a source Atmosphere trusts.");
    }
    get_bytes(http, url).await.ok_or_else(|| anyhow!("Couldn't download that logo. Try another one."))
}

fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8]) {
        Some("image/jpeg")
    } else if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else {
        None
    }
}

pub async fn cover(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    if !allowed_cover(url) {
        bail!("That image isn't from a source Atmosphere trusts.");
    }
    let bytes = get_bytes(http, url).await.ok_or_else(|| anyhow!("Couldn't download that cover. Try another one."))?;
    fit_cover(&bytes)
}

pub fn fit_cover(bytes: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(bytes).map_err(|_| anyhow!("That cover isn't an image Atmosphere can read."))?;
    if sniff(bytes) == Some("image/jpeg") && img.width() <= COVER_EDGE && img.height() <= COVER_EDGE {
        return Ok(bytes.to_vec());
    }
    let img = if img.width() > COVER_EDGE || img.height() > COVER_EDGE { img.resize(COVER_EDGE, COVER_EDGE, image::imageops::FilterType::Lanczos3) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img.to_rgb8())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_album_pages_by_this_artist() {
        assert!(is_album_description("2001 studio album by Daft Punk", "Daft Punk"));
        assert!(is_album_description("2019 EP by Billie Eilish", "Billie Eilish"));
        assert!(!is_album_description("2003 single by Daft Punk", "Daft Punk"));
        assert!(!is_album_description("2001 studio album by Gorillaz", "Daft Punk"));
        assert!(is_album_description("1999 compilation album", "Various Artists"));
        assert_eq!(base("Discovery (Daft Punk album)"), base("Discovery"));
        assert_eq!(base("Homework (25th Anniversary Edition)"), "homework");
    }

    #[test]
    fn trusts_only_known_cover_hosts() {
        assert!(allowed_cover("https://cdn-images.dzcdn.net/images/cover/abc/1000x1000-000000-80-0-0.jpg"));
        assert!(allowed_cover("https://is1-ssl.mzstatic.com/image/thumb/Music/x/1500x1500bb.jpg"));
        assert!(allowed_cover("https://coverartarchive.org/release-group/48117b90-a16e-34ca-a514-19c702df1158/front-1200"));
        assert!(allowed_cover("https://r2.theaudiodb.com/images/media/album/thumb/x.jpg"));
        assert!(!allowed_cover("http://cdn-images.dzcdn.net/x.jpg"));
        assert!(!allowed_cover("https://evil.example/dzcdn.net.jpg"));
        assert!(!allowed_cover("https://dzcdn.net.evil.example/x.jpg"));
    }

    #[test]
    fn trusts_only_known_logo_hosts() {
        assert!(allowed_logo("https://commons.wikimedia.org/wiki/Special:FilePath/Starboy_album_logo.png?width=800"));
        assert!(allowed_logo("https://r2.theaudiodb.com/images/media/artist/logo/x.png"));
        assert!(!allowed_logo("https://commons.wikimedia.org/w/index.php?title=x"));
        assert!(!allowed_logo("http://commons.wikimedia.org/wiki/Special:FilePath/x.png"));
        assert!(!allowed_logo("https://commons.wikimedia.org.evil.example/wiki/Special:FilePath/x.png"));
        assert!(names_album("Starboy album logo", "starboy", "theweeknd") && names_album("Planet Her (Logo)", "planether", "dojacat") && names_album("Positions wordmark 1", "positions", "arianagrande"));
        assert!(names_album("Anti Rihanna. logo", "anti", "rihanna") && !names_album("Antifaschistische Aktion logo", "anti", "rihanna") && !names_album("Anti-doping logo black", "anti", "rihanna"));
        assert!(looks_like_logo("Planet Her (Logo)") && looks_like_logo("Positions wordmark 1") && !looks_like_logo("Positions album"));
    }

    #[test]
    fn large_covers_are_shrunk_to_a_jpeg() {
        let big = image::DynamicImage::new_rgb8(3000, 3000);
        let mut png = std::io::Cursor::new(Vec::new());
        big.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let out = fit_cover(png.get_ref()).unwrap();
        assert_eq!(sniff(&out), Some("image/jpeg"));
        let back = image::load_from_memory(&out).unwrap();
        assert_eq!((back.width(), back.height()), (COVER_EDGE, COVER_EDGE));
    }

    #[tokio::test]
    #[ignore]
    async fn live_finds_discovery() {
        let http = reqwest::Client::builder().user_agent("atmosphere/test (+https://github.com/pvrzz/atmosphere)").build().unwrap();
        let f = find(&http, "Daft Punk", "Discovery", true, true).await;
        assert!(f.bio.as_deref().is_some_and(|b| b.contains("Daft Punk")), "no bio");
        let (covers, pages) = choices(&http, "Discovery Daft Punk", "Daft Punk", "Discovery").await;
        assert!(covers.iter().any(|c| c.source == "Deezer"), "no Deezer covers");
        assert!(covers.iter().any(|c| c.source == "Apple Music"), "no Apple covers");
        assert!(pages.iter().any(|p| p.music), "no album page");
        let bytes = cover(&http, &covers[0].full).await.unwrap();
        assert_eq!(sniff(&bytes), Some("image/jpeg"));
    }

    #[tokio::test]
    #[ignore]
    async fn live_finds_an_album_logo() {
        let http = reqwest::Client::builder().user_agent("atmosphere/test (+https://github.com/pvrzz/atmosphere)").build().unwrap();
        let f = find(&http, "Ariana Grande", "Positions", true, true).await;
        assert!(f.logo.is_some(), "no logo");
        assert!(f.bio_source.as_deref().is_some_and(|u| u.contains("wikipedia.org")), "no Wikipedia bio");
        for (artist, title) in [("Ariana Grande", "Positions"), ("The Weeknd", "Starboy"), ("Doja Cat", "Planet Her (Deluxe)"), ("Rihanna", "ANTI")] {
            let logos = logo_choices(&http, &format!("{title} {artist}"), artist, title).await;
            eprintln!("{title}: {:?}", logos.iter().map(|l| format!("{} [{}]", l.name, l.source)).collect::<Vec<_>>());
            assert!(logos.iter().any(|l| l.source != "TheAudioDB"), "no album logos for {title}");
            assert!(logos.iter().all(|l| l.thumb.starts_with("data:image/") && allowed_logo(&l.full)));
        }
        let bytes = logo(&http, &commons_file("Starboy album logo.png", 800)).await.unwrap();
        assert_eq!(sniff(&bytes), Some("image/png"));
    }
}
