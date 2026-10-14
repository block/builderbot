//! Shared, validated writes to the existing image store (including note videos).

use super::materialize::RemoteMedia;
use crate::store::{Image, Store};
use std::io::Read;
use std::path::Path;

pub(crate) const MAX_IMAGE_SIZE: u64 = 10 * 1024 * 1024;
/// PDF attachments (scans and print-ready files routinely run 10–50 MB) get
/// their own cap; they are stored as-is, and only the PNGs rendered from them
/// for thumbnails and the agent prompt have to satisfy [`MAX_IMAGE_SIZE`].
pub(crate) const MAX_PDF_INPUT_SIZE: u64 = 50 * 1024 * 1024;
pub(crate) const MAX_VIDEO_SIZE: u64 = 100 * 1024 * 1024;

pub(crate) fn size_error(what: &str, cap: u64) -> String {
    format!("{what} exceeds {} MB limit", cap / 1024 / 1024)
}

/// Input cap for a chat attachment, decided from its leading bytes, with the
/// noun its size error names so an oversized PDF is reported as such.
pub(crate) fn attachment_limit(header: &[u8]) -> (u64, &'static str) {
    if super::pdf::is_pdf(header) {
        (MAX_PDF_INPUT_SIZE, "PDF")
    } else {
        (MAX_IMAGE_SIZE, "media")
    }
}
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
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// Cap for a note media reference. PDFs are chat attachments only: the note
/// renderer has no PDF figure, so a `.pdf` reference is refused here rather
/// than stored and shown broken.
pub(crate) fn size_limit(filename: &str, allow_video: bool) -> Result<u64, String> {
    let ext = extension(filename);
    let mime = mime_for_extension(&ext).ok_or("unsupported media format")?;
    if mime == "application/pdf" {
        return Err("PDFs cannot be embedded in notes".into());
    }
    if mime.starts_with("video/") {
        if !allow_video {
            return Err("only images can be uploaded here".into());
        }
        Ok(MAX_VIDEO_SIZE)
    } else {
        Ok(MAX_IMAGE_SIZE)
    }
}

/// Canonical extension for a recognized file signature.
pub(crate) fn sniff_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if super::pdf::is_pdf(bytes) {
        Some("pdf")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if bytes.starts_with(b"\x1a\x45\xdf\xa3") {
        bytes[..bytes.len().min(4096)]
            .windows(4)
            .any(|s| s == b"webm")
            .then_some("webm")
    } else if bytes.len() >= 16 && bytes.get(4..8) == Some(b"ftyp") {
        // ISO base media / QuickTime containers. Reject image-only ftyp
        // brands (AVIF/HEIF), even if the file has been renamed to .mp4.
        match bytes.get(8..12)? {
            b"qt  " => Some("mov"),
            b"isom" | b"iso2" | b"iso3" | b"iso4" | b"iso5" | b"iso6" | b"mp41" | b"mp42"
            | b"avc1" | b"M4V " | b"M4VH" | b"MSNV" | b"dash" => Some("mp4"),
            _ => None,
        }
    } else {
        None
    }
}

pub(crate) fn validate_media(
    filename: &str,
    bytes: &[u8],
    allow_video: bool,
) -> Result<&'static str, String> {
    let cap = size_limit(filename, allow_video)?;
    if bytes.len() as u64 > cap {
        return Err(size_error("media", cap));
    }
    let ext = extension(filename);
    let valid = match (ext.as_str(), sniff_format(bytes)) {
        // Either container extension is used for both brands in practice.
        ("mp4" | "mov", Some("mp4" | "mov")) => true,
        (ext, Some(sniffed)) => mime_for_extension(sniffed) == mime_for_extension(ext),
        (_, None) => false,
    };
    if !valid {
        return Err("file signature does not match its media extension".into());
    }
    Ok(mime_for_extension(&ext).unwrap())
}

/// Chat attachments are stored under the format their bytes actually are.
/// Browsers derive `file.type` from the filename, so a WebP saved from a
/// website as `shot.png` arrives labelled image/png; renaming it to the
/// sniffed extension keeps MIME, extension, and signature in agreement
/// instead of rejecting a file WebKit displays fine.
pub(crate) fn store_sniffed_image(
    store: &Store,
    branch_id: Option<&str>,
    project_id: &str,
    session_id: Option<&str>,
    filename: &str,
    bytes: &[u8],
) -> Result<Image, String> {
    let (cap, what) = attachment_limit(bytes);
    if bytes.len() as u64 > cap {
        return Err(size_error(what, cap));
    }
    // A PDF is stored byte-for-byte so the viewer can show the vector
    // original; thumbnails and the agent prompt render pages from it later.
    // Only this chat/timeline path accepts one: note ingest (`validate_media`)
    // still refuses `.pdf` references.
    if super::pdf::is_pdf(bytes) {
        super::pdf::check(bytes)?;
    }
    let sniffed = sniff_format(bytes).ok_or("unsupported image format")?;
    // Every sniffed extension has a MIME entry.
    let mime = mime_for_extension(sniffed).unwrap();
    if mime.starts_with("video/") {
        return Err("only images can be uploaded here".into());
    }
    let filename = if mime_for_extension(&extension(filename)) == Some(mime) {
        filename.to_owned()
    } else {
        Path::new(filename)
            .with_extension(sniffed)
            .to_string_lossy()
            .into_owned()
    };
    // Already validated above (cap, signature, no video); `validate_media`
    // would refuse the PDF a note cannot embed.
    write_media(
        store, branch_id, project_id, session_id, &filename, mime, bytes,
    )
}

/// Why a media source could not be read. Only [`ReadError::NotFound`] may be
/// answered with a previously saved attachment; a file that exists but is
/// oversized, mislabelled, or not a regular file must stay a visible failure.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReadError {
    NotFound(&'static str),
    Other(String),
}

impl From<ReadError> for String {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::NotFound(reason) => reason.to_owned(),
            ReadError::Other(reason) => reason,
        }
    }
}

impl From<String> for ReadError {
    fn from(reason: String) -> Self {
        ReadError::Other(reason)
    }
}

impl From<&str> for ReadError {
    fn from(reason: &str) -> Self {
        ReadError::Other(reason.to_owned())
    }
}

pub(crate) fn read_local(path: &Path, cap: u64) -> Result<Vec<u8>, ReadError> {
    let (file, len) = open_regular(path)?;
    if len > cap {
        return Err(size_error("media", cap).into());
    }
    read_capped(file, Vec::new(), cap)
}

/// [`read_local`] for a chat attachment, whose cap depends on what the file
/// is (see [`attachment_limit`]). The signature is read before the body so an
/// oversized image is still refused from its metadata, without buffering
/// anything like [`MAX_PDF_INPUT_SIZE`] first.
pub(crate) fn read_local_attachment(path: &Path) -> Result<Vec<u8>, ReadError> {
    let (mut file, len) = open_regular(path)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(PDF_SIGNATURE_LEN)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read media file")?;
    let (cap, what) = attachment_limit(&bytes);
    if len > cap {
        return Err(size_error(what, cap).into());
    }
    read_capped(file, bytes, cap)
}

const PDF_SIGNATURE_LEN: u64 = b"%PDF-".len() as u64;

/// Open a regular file for reading and return it with its current length.
fn open_regular(path: &Path) -> Result<(std::fs::File, u64), ReadError> {
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
        .map_err(|_| ReadError::NotFound("media file is unavailable"))?;
    let metadata = file.metadata().map_err(|_| "cannot inspect media file")?;
    if !metadata.is_file() {
        return Err("media must be a regular file".into());
    }
    Ok((file, metadata.len()))
}

/// Read the rest of `file` after `bytes` (already read from it), refusing a
/// file that grew past `cap` since its metadata was checked.
fn read_capped(file: std::fs::File, mut bytes: Vec<u8>, cap: u64) -> Result<Vec<u8>, ReadError> {
    file.take(cap + 1 - bytes.len() as u64)
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
    write_media(
        store, branch_id, project_id, session_id, filename, mime, bytes,
    )
}

/// Write validated bytes under their MIME: a unique filename on the branch,
/// the file, then the row (the file is removed if the insert fails).
fn write_media(
    store: &Store,
    branch_id: Option<&str>,
    project_id: &str,
    session_id: Option<&str>,
    filename: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<Image, String> {
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
    crate::store::images::remove_image_files(&image.project_id, &image.id, &image.filename);
    Ok(())
}

/// Probe-and-read script for remote sources. Positional shell arguments handle
/// spaces/quotes without interpolation. It exits 1 when the path does not
/// exist and 2, with [`REMOTE_NOT_REGULAR`] on stderr, when it exists but is
/// not a regular file; `ws_exec` reports only success and stderr, so the
/// message is what tells the two apart. Output is limited before it crosses
/// the workspace boundary, and devices/FIFOs are rejected instead of blocking
/// on a non-file read.
pub(crate) const REMOTE_READ_SCRIPT: &str = "cd -- \"$2\" || exit 1; media_path=$1; case $media_path in '~/'*) media_path=\"$HOME/${media_path#??}\";; esac; test -e \"$media_path\" || exit 1; test -f \"$media_path\" || { echo 'staged-note-media: not a regular file' >&2; exit 2; }; head -c \"$3\" -- \"$media_path\"";
pub(crate) const REMOTE_NOT_REGULAR: &str = "staged-note-media: not a regular file";

pub(crate) fn read_candidate(
    target: &str,
    workspace: Option<&str>,
    cwd: &Path,
    cap: u64,
    remote: &dyn RemoteMedia,
) -> Result<Vec<u8>, ReadError> {
    if let Some(workspace) = workspace {
        let output = remote
            .exec_output(
                workspace,
                &[
                    "sh",
                    "-c",
                    REMOTE_READ_SCRIPT,
                    "staged-note-media",
                    target,
                    &cwd.to_string_lossy(),
                    &(cap + 1).to_string(),
                ],
            )
            // An unreachable workspace reads as a missing source so a saved
            // attachment is not deleted over a transient failure.
            .map_err(|_| ReadError::NotFound("remote media file is unavailable"))?;
        if !output.success {
            if String::from_utf8_lossy(&output.stderr).contains(REMOTE_NOT_REGULAR) {
                return Err("remote media must be a regular file".into());
            }
            return Err(ReadError::NotFound("remote media file is unavailable"));
        }
        let bytes = output.stdout;
        if bytes.len() as u64 > cap {
            return Err(size_error("media", cap).into());
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
