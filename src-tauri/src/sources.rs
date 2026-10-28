//! Live public archive adapters. Sampling is intentionally source-balanced, not
//! uniform over the Internet; see docs/randomness.md for the practical biases.
use crate::network::{validate_url, SafeClient};
use anyhow::{bail, Context, Result};
use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;
use serde_json::Value;
use url::Url;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub url: String,
    pub filename: String,
    pub expected_size: Option<u64>,
    pub mime: Option<String>,
    /// Lowercase, forty-character SHA-1 digest supplied by the archive.
    pub sha1: Option<String>,
    pub source: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Image,
    Text,
    Audio,
    Video,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Source {
    Commons,
    Archive,
}

fn source_order<R: Rng + ?Sized>(rng: &mut R) -> [Source; 2] {
    if rng.gen() {
        [Source::Commons, Source::Archive]
    } else {
        [Source::Archive, Source::Commons]
    }
}

impl Kind {
    fn archive_type(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Text => "texts",
            Self::Audio => "audio",
            Self::Video => "movies",
        }
    }
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

fn file_kind(name: &str) -> Option<Kind> {
    match extension(name).as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "tif" | "tiff" => Some(Kind::Image),
        "txt" | "pdf" | "html" | "htm" => Some(Kind::Text),
        "mp3" | "ogg" | "oga" | "flac" | "wav" => Some(Kind::Audio),
        "mp4" | "webm" | "ogv" | "mov" => Some(Kind::Video),
        _ => None,
    }
}

pub fn supported_extension(name: &str) -> bool {
    file_kind(name).is_some()
}

fn mime_for(name: &str) -> Option<String> {
    Some(
        match extension(name).as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "tif" | "tiff" => "image/tiff",
            "pdf" => "application/pdf",
            "txt" => "text/plain",
            "html" | "htm" => "text/html",
            "mp3" => "audio/mpeg",
            "ogg" | "oga" => "audio/ogg",
            "flac" => "audio/flac",
            "wav" => "audio/wav",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "ogv" => "video/ogg",
            "mov" => "video/quicktime",
            _ => return None,
        }
        .to_owned(),
    )
}

fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn hex_sha1(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    (text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

/// Imageinfo returns forty-character hexadecimal SHA-1. Also accept the base-36
/// representation used internally by MediaWiki, without lossy arithmetic.
fn commons_sha1(value: &Value) -> Option<String> {
    if let Some(digest) = hex_sha1(value) {
        return Some(digest);
    }
    let text = value.as_str()?;
    if text.is_empty() || text.len() > 31 {
        return None;
    }
    let mut bytes = [0_u8; 20];
    for character in text.chars() {
        let mut carry = character.to_digit(36)?;
        for byte in bytes.iter_mut().rev() {
            carry += u32::from(*byte) * 36;
            *byte = carry as u8;
            carry >>= 8;
        }
        if carry != 0 {
            return None;
        }
    }
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn choose_balanced<R: Rng + ?Sized>(mut files: Vec<Candidate>, rng: &mut R) -> Option<Candidate> {
    // Choose a format first so thumbnail/derivative counts do not dictate the format.
    let mut formats: Vec<_> = files.iter().map(|file| extension(&file.filename)).collect();
    formats.sort_unstable();
    formats.dedup();
    let selected = formats.choose(rng)?;
    files.retain(|file| extension(&file.filename) == *selected);
    files.choose(rng).cloned()
}

struct Commons;

impl Commons {
    async fn discover(client: &SafeClient) -> Result<Candidate> {
        let metadata = client.json("https://commons.wikimedia.org/w/api.php?action=query&generator=random&grnnamespace=6&grnlimit=12&grnfilterredir=nonredirects&prop=imageinfo&iiprop=url%7Csize%7Cmime%7Csha1&format=json&formatversion=2").await?;
        choose_balanced(Self::candidates(&metadata), &mut OsRng)
            .context("No supported public files were present in the Commons sample")
    }

    fn candidates(metadata: &Value) -> Vec<Candidate> {
        metadata["query"]["pages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|page| {
                let info = page["imageinfo"].as_array()?.first()?;
                let raw_url = info["url"].as_str()?;
                let url = validate_url(raw_url).ok()?;
                if url.host_str() != Some("upload.wikimedia.org") {
                    return None;
                }
                let filename = page["title"].as_str()?.strip_prefix("File:")?.to_owned();
                if !supported_extension(&filename) {
                    return None;
                }
                Some(Candidate {
                    url: url.into(),
                    filename,
                    expected_size: number(&info["size"]),
                    mime: info["mime"].as_str().map(str::to_owned),
                    sha1: commons_sha1(&info["sha1"]),
                    source: "Wikimedia Commons",
                })
            })
            .collect()
    }
}

struct InternetArchive;

impl InternetArchive {
    async fn discover(client: &SafeClient) -> Result<Candidate> {
        let kind = *[Kind::Image, Kind::Text, Kind::Audio, Kind::Video]
            .choose(&mut OsRng)
            .expect("nonempty media groups");
        // Random decade partitions reach beyond the first popular search page.
        // One quarter of requests omit the date filter, including undated material.
        let decade = (OsRng.gen_range(0..23) * 10) + 1800;
        let year_query = if OsRng.gen_ratio(3, 4) {
            format!(" AND year:[{decade} TO {}]", decade + 9)
        } else {
            String::new()
        };
        let query = format!(
            "mediatype:{} AND NOT access-restricted-item:true AND NOT collection:malwaremuseum{year_query}",
            kind.archive_type()
        );
        let mut url = Url::parse("https://archive.org/advancedsearch.php")?;
        url.query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("fl[]", "identifier")
            .append_pair("sort[]", "identifier asc")
            .append_pair("rows", "0")
            .append_pair("output", "json");
        let count = client.json(url.as_str()).await?;
        let total = number(&count["response"]["numFound"])
            .filter(|count| *count > 0)
            .context("No accessible archive items in the selected sample")?;
        // Bound the index depth: the search endpoint does not guarantee unlimited
        // deep paging. Decade/media partitions mitigate this documented bias.
        let page = OsRng.gen_range(1..=total.min(10_000));
        url.set_query(None);
        url.query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("fl[]", "identifier")
            .append_pair("sort[]", "identifier asc")
            .append_pair("rows", "1")
            .append_pair("page", &page.to_string())
            .append_pair("output", "json");
        let results = client.json(url.as_str()).await?;
        let identifier = results["response"]["docs"]
            .as_array()
            .and_then(|docs| docs.first())
            .and_then(|doc| doc["identifier"].as_str())
            .filter(|identifier| valid_identifier(identifier))
            .context("The archive returned an invalid item identifier")?;
        let metadata = client
            .json(&format!("https://archive.org/metadata/{identifier}"))
            .await?;
        choose_balanced(Self::candidates(identifier, &metadata, kind), &mut OsRng)
            .context("No supported public files were present in the archive item")
    }

    fn candidates(identifier: &str, metadata: &Value, kind: Kind) -> Vec<Candidate> {
        if !valid_identifier(identifier)
            || metadata["is_dark"] == true
            || metadata["metadata"]["access-restricted-item"] == "true"
            || metadata["metadata"]["access-restricted-item"] == true
        {
            return Vec::new();
        }
        metadata["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|file| {
                if file["private"] == true || file["private"] == "true" {
                    return None;
                }
                let filename = file["name"].as_str()?;
                if file_kind(filename) != Some(kind)
                    || filename.split('/').any(|part| part == ".." || part == ".")
                    || filename.contains('\\')
                    || filename.starts_with('/')
                {
                    return None;
                }
                let mut url = Url::parse("https://archive.org/download/").ok()?;
                url.path_segments_mut()
                    .ok()?
                    .pop_if_empty()
                    .push(identifier)
                    .extend(filename.split('/'));
                Some(Candidate {
                    url: url.into(),
                    filename: filename.rsplit('/').next()?.to_owned(),
                    expected_size: number(&file["size"]),
                    mime: mime_for(filename),
                    sha1: hex_sha1(&file["sha1"]),
                    source: "Internet Archive",
                })
            })
            .collect()
    }
}

fn valid_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier.len() <= 200
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        && !matches!(identifier, "." | "..")
}

/// Randomize the initial source, then alternate on failure. Retry count and total
/// discovery time are bounded; a source outage cannot trap the interface forever.
pub async fn discover(client: &SafeClient) -> Result<Candidate> {
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let order = source_order(&mut OsRng);
        let mut last_error = None;
        for attempt in 0..6 {
            let result = match order[attempt % order.len()] {
                Source::Commons => Commons::discover(client).await,
                Source::Archive => InternetArchive::discover(client).await,
            };
            match result {
                Ok(candidate) => return Ok(candidate),
                Err(error) => last_error = Some(error),
            }
            if attempt < 5 {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }
        bail!(
            "No public source returned a supported file: {}",
            last_error.expect("six attempts")
        );
    })
    .await
    .context("Public file discovery timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use serde_json::json;

    #[test]
    fn executable_and_archive_formats_are_excluded() {
        for name in [
            "a.exe",
            "a.js",
            "a.svg",
            "a.zip",
            "a.iso",
            "a.pdf.exe",
            "a.desktop",
            "a.sh",
            "a",
        ] {
            assert!(!supported_extension(name));
        }
        for name in [
            "photo.JPEG",
            "book.pdf",
            "note.txt",
            "old.html",
            "sound.flac",
            "film.webm",
        ] {
            assert!(supported_extension(name));
        }
    }

    #[test]
    fn source_selection_reaches_both_adapters_and_always_has_fallback() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let mut commons_first = 0;
        for _ in 0..1000 {
            let order = source_order(&mut rng);
            assert_ne!(order[0], order[1]);
            commons_first += usize::from(order[0] == Source::Commons);
        }
        assert!((400..600).contains(&commons_first));
    }

    #[test]
    fn commons_metadata_handles_missing_fields_and_untrusted_urls() {
        let metadata = json!({"query":{"pages":[
            {"title":"File:picture.jpg","imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/a/ab/picture.jpg","mime":"image/jpeg","size":42,"sha1":"1"}]},
            {"title":"File:bad.jpg","imageinfo":[{"url":"https://127.0.0.1/a"}]},
            {"title":"File:empty.jpg"},
            {"title":"File:program.exe","imageinfo":[{"url":"https://upload.wikimedia.org/a"}]}
        ]}});
        let candidates = Commons::candidates(&metadata);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].expected_size, Some(42));
        assert_eq!(
            candidates[0].sha1.as_deref(),
            Some("0000000000000000000000000000000000000001")
        );
        assert!(Commons::candidates(&json!({})).is_empty());
    }

    #[test]
    fn archive_metadata_rejects_traversal_and_filters_by_kind() {
        let metadata = json!({"files":[
            {"name":"folder/my book.pdf","size":"100","sha1":"0123456789abcdef0123456789abcdef01234567"},
            {"name":"../escape.pdf"}, {"name":"private.pdf","private":"true"},
            {"name":"thumbnail.jpg"}, {"name":"run.exe"}
        ]});
        let candidates = InternetArchive::candidates("item-123", &metadata, Kind::Text);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].filename, "my book.pdf");
        assert_eq!(
            candidates[0].url,
            "https://archive.org/download/item-123/folder/my%20book.pdf"
        );
        assert_eq!(candidates[0].expected_size, Some(100));
        assert!(InternetArchive::candidates("../bad", &metadata, Kind::Text).is_empty());
        assert!(InternetArchive::candidates(
            "item",
            &json!({"is_dark":true,"files":[{"name":"a.pdf"}]}),
            Kind::Text
        )
        .is_empty());
    }

    #[test]
    fn base36_sha1_conversion_is_exact_and_bounded() {
        assert_eq!(
            commons_sha1(&json!("0123456789ABCDEF0123456789ABCDEF01234567")),
            Some("0123456789abcdef0123456789abcdef01234567".into())
        );
        assert_eq!(commons_sha1(&json!("35")), Some(format!("{:040x}", 113)));
        assert_eq!(
            commons_sha1(&json!("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")),
            None
        );
        assert_eq!(commons_sha1(&json!("?")), None);
        assert_eq!(hex_sha1(&json!("oops")), None);
    }

    #[test]
    fn format_selection_does_not_weight_duplicate_formats() {
        let candidate = |name: &str| Candidate {
            url: "https://archive.org/a".into(),
            filename: name.into(),
            expected_size: None,
            mime: None,
            sha1: None,
            source: "fixture",
        };
        let mut files = vec![candidate("one.pdf")];
        files.extend((0..100).map(|index| candidate(&format!("{index}.txt"))));
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let pdf_count = (0..1000)
            .filter(|_| choose_balanced(files.clone(), &mut rng).unwrap().filename == "one.pdf")
            .count();
        assert!((400..600).contains(&pdf_count));
        assert!(choose_balanced(Vec::new(), &mut rng).is_none());
    }
}
