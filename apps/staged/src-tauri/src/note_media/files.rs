//! Shared, validated writes to the existing image store (including note videos).

use crate::store::{Image, Store};
use std::io::Read;
use std::path::Path;

pub(crate) const MAX_IMAGE_SIZE: u64 = 10 * 1024 * 1024;
pub(crate) const MAX_VIDEO_SIZE: u64 = 100 * 1024 * 1024;
pub(crate) const MAX_REMOTE_VIDEO_SIZE: i64 = 20 * 1024 * 1024;

pub(crate) fn extension(filename: &str) -> String {
    Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub(crate) fn mime_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

pub(crate) fn size_limit(filename: &str, allow_video: bool) -> Result<u64, String> {
    let ext = extension(filename);
    let mime = mime_for_extension(&ext).ok_or("unsupported media format")?;
    if mime.starts_with("video/") {
        if !allow_video {
            return Err("only images can be uploaded here".into());
        }
        Ok(MAX_VIDEO_SIZE)
    } else {
        Ok(MAX_IMAGE_SIZE)
    }
}

pub(crate) fn validate_media(
    filename: &str,
    bytes: &[u8],
    allow_video: bool,
) -> Result<&'static str, String> {
    let cap = size_limit(filename, allow_video)?;
    if bytes.len() as u64 > cap {
        return Err(format!("media exceeds {} MB limit", cap / 1024 / 1024));
    }
    let ext = extension(filename);
    let valid = match ext.as_str() {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        "mp4" | "mov" => {
            // ISO base media / QuickTime containers. Reject image-only ftyp
            // brands (AVIF/HEIF), even if the file has been renamed to .mp4.
            bytes.get(4..8) == Some(b"ftyp")
                && bytes.get(8..12).is_some_and(|brand| {
                    matches!(
                        brand,
                        b"isom"
                            | b"iso2"
                            | b"iso3"
                            | b"iso4"
                            | b"iso5"
                            | b"iso6"
                            | b"mp41"
                            | b"mp42"
                            | b"avc1"
                            | b"M4V "
                            | b"M4VH"
                            | b"qt  "
                            | b"MSNV"
                            | b"dash"
                    )
                })
                && bytes.len() >= 16
        }
        "webm" => {
            bytes.starts_with(b"\x1a\x45\xdf\xa3")
                && bytes[..bytes.len().min(4096)]
                    .windows(4)
                    .any(|s| s == b"webm")
        }
        _ => false,
    };
    if !valid {
        return Err("file signature does not match its media extension".into());
    }
    Ok(mime_for_extension(&ext).unwrap())
}

pub(crate) fn read_local(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Opening a FIFO without this flag would block before metadata could
        // reject it. Regular files ignore O_NONBLOCK.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "media file is unavailable")?;
    let metadata = file.metadata().map_err(|_| "cannot inspect media file")?;
    if !metadata.is_file() {
        return Err("media must be a regular file".into());
    }
    if metadata.len() > cap {
        return Err(format!("media exceeds {} MB limit", cap / 1024 / 1024));
    }
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read media file")?;
    if bytes.len() as u64 > cap {
        return Err("media grew beyond size limit".into());
    }
    Ok(bytes)
}

/// Validate before writing; remove the copy if insertion fails. MIME is always
/// derived from the validated extension, never trusted from a client request.
pub(crate) fn store_media_file(
    store: &Store,
    branch_id: Option<&str>,
    project_id: &str,
    session_id: Option<&str>,
    filename: &str,
    bytes: &[u8],
    allow_video: bool,
) -> Result<Image, String> {
    let mime = validate_media(filename, bytes, allow_video)?;
    let filename = Path::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid media filename")?;
    // Normalize the extension so stored references, paths, and MIME agree.
    let filename = Path::new(filename)
        .with_extension(extension(filename))
        .to_string_lossy()
        .into_owned();
    let filename = store
        .unique_image_filename(branch_id, project_id, &filename)
        .map_err(|e| e.to_string())?;
    let mut image = Image::new(
        branch_id,
        project_id,
        &filename,
        mime,
        bytes.len() as i64,
        false,
    );
    image.session_id = session_id.map(str::to_owned);
    let path = crate::store::images::image_file_path(project_id, &image.id, &filename)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::write(&path, bytes) {
        let _ = std::fs::remove_file(&path);
        return Err(e.to_string());
    }
    if let Err(e) = store.create_image(&image) {
        let _ = std::fs::remove_file(&path);
        return Err(e.to_string());
    }
    Ok(image)
}

pub(crate) fn delete_media_file(store: &Store, image: &Image) -> Result<(), String> {
    store.delete_image(&image.id).map_err(|e| e.to_string())?;
    if let Ok(path) =
        crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
    {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("Cannot remove media {}: {e}", image.id);
            }
        }
    }
    Ok(())
}

pub(crate) fn read_candidate(
    target: &str,
    workspace: Option<&str>,
    cwd: &Path,
    cap: u64,
) -> Result<Vec<u8>, String> {
    if let Some(workspace) = workspace {
        // Positional shell arguments handle spaces/quotes without interpolation.
        // Limit output before it crosses the workspace boundary, and reject
        // devices/FIFOs instead of blocking on a non-file read.
        let script = "cd -- \"$2\" || exit; media_path=$1; case $media_path in '~/'*) media_path=\"$HOME/${media_path#??}\";; esac; test -f \"$media_path\" || exit 1; head -c \"$3\" -- \"$media_path\"";
        let bytes = crate::blox::ws_exec_bytes(
            workspace,
            &[
                "sh",
                "-c",
                script,
                "staged-note-media",
                target,
                &cwd.to_string_lossy(),
                &(cap + 1).to_string(),
            ],
        )
        .map_err(|_| "remote media file is unavailable")?;
        if bytes.len() as u64 > cap {
            return Err(format!("media exceeds {} MB limit", cap / 1024 / 1024));
        }
        Ok(bytes)
    } else {
        let path = if let Some(rest) = target.strip_prefix("~/") {
            dirs::home_dir()
                .ok_or("home directory is unavailable")?
                .join(rest)
        } else {
            cwd.join(target)
        };
        read_local(&path, cap)
    }
}

/// Markdown destinations commonly percent-encode spaces. Decode them for
/// filesystem access while leaving the markdown itself untouched until ingest.
pub(crate) fn local_target(target: &str) -> Result<String, String> {
    if target
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("file:"))
    {
        return reqwest::Url::parse(target)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| "invalid file URL".into());
    }
    // Stored IDs are validated literally, without accepting encoded aliases.
    if target.starts_with("staged-media:") {
        return Ok(target.to_owned());
    }
    let mut decoded = Vec::new();
    let bytes = target.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(a), Some(b)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                decoded.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(decoded).map_err(|_| "invalid UTF-8 media path".into())
}
