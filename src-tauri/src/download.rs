//! Streaming, bounded downloads. Remote bytes are never executed or rendered.
use crate::{network::SafeClient, sources::Candidate};
use anyhow::{bail, ensure, Context, Result};
use futures_util::StreamExt;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub const CONFIRM_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const SPACE_RESERVE: u64 = 16 * 1024 * 1024;
const STALL_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Prepared {
    pub candidate: Candidate,
    pub total: Option<u64>,
    response: reqwest::Response,
}

pub struct Receipt {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

pub fn requires_confirmation(size: Option<u64>) -> bool {
    size.is_none_or(|n| n > CONFIRM_BYTES)
}

pub async fn prepare(client: &SafeClient, candidate: Candidate) -> Result<Prepared> {
    ensure!(
        candidate.expected_size.is_none_or(|n| n <= MAX_BYTES),
        "File exceeds the 2 GiB limit"
    );
    let response = client.get(&candidate.url).await?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "Expected a complete file response"
    );
    ensure!(
        !response
            .headers()
            .contains_key(reqwest::header::CONTENT_ENCODING),
        "Encoded transfers are unsupported"
    );
    let length = response.content_length();
    if let (Some(expected), Some(actual)) = (candidate.expected_size, length) {
        ensure!(expected == actual, "File size changed since discovery");
    }
    let total = length.or(candidate.expected_size);
    ensure!(
        total.is_none_or(|n| n <= MAX_BYTES),
        "File exceeds the 2 GiB limit"
    );
    let header_mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim());
    let filename = sanitize_filename(&candidate.filename)?;
    validate_mime(&filename, header_mime)?;
    if let Some(mime) = candidate.mime.as_deref() {
        validate_mime(&filename, Some(mime))?;
    }
    Ok(Prepared {
        candidate,
        total,
        response,
    })
}

pub async fn download(
    prepared: Prepared,
    destination: &Path,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Receipt> {
    // Selecting around the complete future makes every await cancellable; RAII
    // removes the private temporary file on cancellation, errors or unwinding.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("Download cancelled"),
        result = tokio::time::timeout(Duration::from_secs(15 * 60), save(prepared, destination, &mut progress)) => {
            result.context("Download exceeded the 15 minute limit")?
        }
    }
}

async fn save(
    prepared: Prepared,
    destination: &Path,
    progress: &mut impl FnMut(u64, Option<u64>),
) -> Result<Receipt> {
    ensure!(
        prepared.total.is_none_or(|n| n <= MAX_BYTES),
        "File exceeds the 2 GiB limit"
    );
    let filename = sanitize_filename(&prepared.candidate.filename)?;
    tokio::fs::create_dir_all(destination)
        .await
        .context("Cannot create Downloads directory")?;
    check_space(destination, prepared.total.unwrap_or(CONFIRM_BYTES))?;
    let temp = tempfile::Builder::new()
        .prefix(".boton-")
        .suffix(".part")
        .tempfile_in(destination)?;
    let mut writer = tokio::fs::File::from_std(temp.reopen()?);
    let mut stream = prepared.response.bytes_stream();
    let mut bytes = 0u64;
    let mut prefix = Vec::with_capacity(8192);
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut last_progress = Instant::now();
    let mut next_space_check = 32 * 1024 * 1024;
    while let Some(chunk) = tokio::time::timeout(STALL_TIMEOUT, stream.next())
        .await
        .context("Download stalled")?
    {
        let chunk = chunk.context("File transfer interrupted")?;
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .context("Invalid file length")?;
        ensure!(bytes <= MAX_BYTES, "File exceeds the 2 GiB limit");
        ensure!(
            prepared.total.is_none_or(|n| bytes <= n),
            "Server sent more data than declared"
        );
        if prefix.len() < 8192 {
            let count = (8192 - prefix.len()).min(chunk.len());
            prefix.extend_from_slice(&chunk[..count]);
        }
        sha1.update(&chunk);
        sha256.update(&chunk);
        writer
            .write_all(&chunk)
            .await
            .context("Cannot write downloaded file")?;
        if bytes >= next_space_check {
            check_space(
                destination,
                prepared
                    .total
                    .map_or(32 * 1024 * 1024, |n| n.saturating_sub(bytes)),
            )?;
            next_space_check = bytes + 32 * 1024 * 1024;
        }
        if last_progress.elapsed() >= Duration::from_millis(120) {
            progress(bytes, prepared.total);
            last_progress = Instant::now();
        }
    }
    ensure!(bytes > 0, "The file was empty");
    ensure!(
        prepared.total.is_none_or(|n| n == bytes),
        "Incomplete download"
    );
    validate_signature(&filename, &prefix)?;
    if let Some(expected) = &prepared.candidate.sha1 {
        ensure!(
            expected.len() == 40 && expected.bytes().all(|c| c.is_ascii_hexdigit()),
            "Invalid source checksum"
        );
        ensure!(
            format!("{:x}", sha1.finalize()).eq_ignore_ascii_case(expected),
            "Source checksum mismatch"
        );
    }
    writer.flush().await?;
    writer.sync_all().await?;
    drop(writer);
    let path = finalize(temp, destination, &filename)?;
    progress(bytes, prepared.total);
    Ok(Receipt {
        path,
        bytes,
        sha256: format!("{:x}", sha256.finalize()),
    })
}

fn check_space(directory: &Path, required: u64) -> Result<()> {
    let available = fs2::available_space(directory).context("Cannot check available disk space")?;
    ensure!(
        available >= required.saturating_add(SPACE_RESERVE),
        "Not enough free space in Downloads"
    );
    Ok(())
}

fn finalize(mut temp: NamedTempFile, directory: &Path, filename: &str) -> Result<PathBuf> {
    let p = Path::new(filename);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .context("Invalid filename")?;
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .context("Missing extension")?;
    for suffix in 0..10_000 {
        let name = if suffix == 0 {
            filename.to_owned()
        } else {
            format!("{stem} ({suffix}).{ext}")
        };
        let target = directory.join(name);
        match temp.persist_noclobber(&target) {
            Ok(_) => return Ok(target),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temp = error.file
            }
            Err(error) => return Err(error.error).context("Cannot finalize the downloaded file"),
        }
    }
    bail!("Too many files with the same name")
}

pub fn sanitize_filename(raw: &str) -> Result<String> {
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let (stem, extension) = leaf
        .rsplit_once('.')
        .context("Unsupported file extension")?;
    let extension = extension.to_ascii_lowercase();
    ensure!(
        allowed_mimes(&extension).is_some(),
        "Unsupported file extension"
    );
    let mut name = String::new();
    for c in stem.chars() {
        if name.len() + c.len_utf8() > 140 {
            break;
        }
        if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
            name.push(c);
        } else if !c.is_control() {
            name.push('_');
        }
    }
    let name = name.trim_matches([' ', '.']);
    let name = if name.is_empty() { "unexpected" } else { name };
    // Prefix Windows device basenames even when running on Linux.
    let upper = name.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            upper.strip_prefix(prefix).is_some_and(|suffix| {
                let mut chars = suffix.chars();
                // Win32 also recognizes the ISO-8859-1 superscript digits as
                // device numbers, even when a filename extension follows.
                matches!(chars.next(), Some('0'..='9' | '¹' | '²' | '³')) && chars.next().is_none()
            })
        });
    Ok(format!(
        "{}{name}.{extension}",
        if reserved { "_" } else { "" }
    ))
}

fn allowed_mimes(ext: &str) -> Option<&'static [&'static str]> {
    Some(match ext {
        "jpg" | "jpeg" => &["image/jpeg"],
        "png" => &["image/png"],
        "gif" => &["image/gif"],
        "webp" => &["image/webp"],
        "tif" | "tiff" => &["image/tiff"],
        "bmp" => &["image/bmp", "image/x-ms-bmp"],
        "pdf" => &["application/pdf"],
        "txt" | "text" => &["text/plain"],
        "html" | "htm" => &["text/html"],
        "csv" => &["text/csv", "text/plain"],
        "json" => &["application/json", "text/plain"],
        "xml" => &["application/xml", "text/xml", "text/plain"],
        "mp3" => &["audio/mpeg", "audio/mp3"],
        "wav" => &["audio/x-wav", "audio/wav", "audio/vnd.wave"],
        "flac" => &["audio/flac", "audio/x-flac"],
        "ogg" | "oga" => &["audio/ogg", "application/ogg"],
        "ogv" => &["video/ogg", "application/ogg"],
        "mp4" | "m4v" => &["video/mp4", "video/x-m4v"],
        "webm" => &["video/webm", "audio/webm"],
        "mov" => &["video/quicktime"],
        _ => return None,
    })
}

pub fn validate_mime(filename: &str, mime: Option<&str>) -> Result<()> {
    let ext = filename.rsplit('.').next().unwrap_or("");
    let allowed = allowed_mimes(ext).context("Unsupported extension")?;
    if let Some(mime) = mime {
        let mime = mime
            .split(';')
            .next()
            .unwrap_or(mime)
            .trim()
            .to_ascii_lowercase();
        ensure!(
            mime == "application/octet-stream" || allowed.contains(&mime.as_str()),
            "File media type does not match its name"
        );
    }
    Ok(())
}

fn validate_signature(filename: &str, prefix: &[u8]) -> Result<()> {
    ensure!(
        !prefix.starts_with(b"MZ")
            && !prefix.starts_with(b"\x7fELF")
            && !prefix.starts_with(b"#!")
            && !prefix.starts_with(b"PK\x03\x04"),
        "Executable or archive payload rejected"
    );
    let ext = filename.rsplit('.').next().unwrap_or("");
    if matches!(
        ext,
        "txt" | "text" | "html" | "htm" | "csv" | "json" | "xml"
    ) {
        // The sniff prefix can end mid-codepoint. Permit only that UTF-8 error.
        if let Err(e) = std::str::from_utf8(prefix) {
            ensure!(e.error_len().is_none(), "Text contains non-UTF-8 data");
        }
        ensure!(!prefix.contains(&0), "Binary payload disguised as text");
        return Ok(());
    }
    let kind = infer::get(prefix).context("File signature was not recognized")?;
    // Ogg's container magic doesn't distinguish video from audio reliably.
    if matches!(ext, "ogg" | "oga" | "ogv") && kind.mime_type().ends_with("/ogg") {
        return Ok(());
    }
    validate_mime(filename, Some(kind.mime_type()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_names_stay_single_safe_components() {
        assert_eq!(sanitize_filename("../../CON.PDF").unwrap(), "_CON.pdf");
        assert_eq!(sanitize_filename("COM¹.txt").unwrap(), "_COM¹.txt");
        assert_eq!(sanitize_filename("LPT².txt").unwrap(), "_LPT².txt");
        assert_eq!(sanitize_filename("C:\\evil\\nice.jpg").unwrap(), "nice.jpg");
        assert!(sanitize_filename("x.exe").is_err());
        assert!(!sanitize_filename("a\u{202e}b.txt")
            .unwrap()
            .contains('\u{202e}'));
        assert!(
            sanitize_filename(&format!("{}.txt", "é".repeat(200)))
                .unwrap()
                .len()
                < 150
        );
    }
    #[test]
    fn size_confirmation_cannot_be_evaded_by_unknown_length() {
        assert!(requires_confirmation(None));
        assert!(!requires_confirmation(Some(CONFIRM_BYTES)));
        assert!(requires_confirmation(Some(CONFIRM_BYTES + 1)));
    }
    #[test]
    fn deceptive_content_is_rejected() {
        assert!(validate_mime("photo.jpg", Some("text/html")).is_err());
        assert!(validate_signature("photo.jpg", b"MZ executable").is_err());
        assert!(validate_signature("file.txt", b"\x7fELF binary").is_err());
        assert!(validate_signature("file.txt", b"PK\x03\x04 zip").is_err());
        assert!(validate_signature("file.txt", b"hello\0world").is_err());
        assert!(validate_signature("file.txt", b"hello world").is_ok());
    }
    #[test]
    fn duplicate_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), b"original").unwrap();
        let mut temp = NamedTempFile::new_in(dir.path()).unwrap();
        std::io::Write::write_all(&mut temp, b"new").unwrap();
        let result = finalize(temp, dir.path(), "hello.txt").unwrap();
        assert_eq!(result.file_name().unwrap(), "hello (1).txt");
        assert_eq!(
            std::fs::read(dir.path().join("hello.txt")).unwrap(),
            b"original"
        );
        assert_eq!(std::fs::read(result).unwrap(), b"new");
    }
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod streaming_tests;
