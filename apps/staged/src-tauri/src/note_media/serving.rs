//! ID-only media lookup and seek-based responses for the desktop URI protocol.
//!
//! Images and PDFs reach the desktop webview through this protocol. Video
//! does too, but only as the fallback for [`super::stream`]: wry hands every
//! response to the webview as one buffer, through the app main thread, so a
//! request with no `Range` header (WebKit sends one for a WebM) reads the
//! whole file into memory here, and WebKit asks for an MP4 one sample at a
//! time.

use super::refs::parse_media_file;
use crate::store::Store;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tauri::http::{header, Method, Request, Response, StatusCode};

/// Where the app's store is fetched from when a file is not in the
/// [`MediaIndex`] yet. The error is the status to answer with.
pub(crate) type StoreSource = dyn Fn() -> Result<Arc<Store>, StatusCode> + Send + Sync;

/// Where each served file is and what it is, by `<id>.<ext>`, so that only
/// the first request for a file asks the store. A store read waits for a
/// connection (it once shared the one the rest of the app writes through;
/// now it waits for one of a few read-only ones), and WebKit sends a video's
/// requests one after another, so a wait there used to be paid by each of the
/// hundreds of requests behind a single seek.
///
/// An id names bytes that never change, but the file moves when its project
/// does and goes when its last reference is deleted. An entry is therefore
/// only trusted while its file is still there (one `stat`, no lock shared
/// with the app); otherwise the store is asked again.
#[derive(Default)]
pub(crate) struct MediaIndex {
    files: RwLock<HashMap<String, (PathBuf, String)>>,
}

/// Entries are a path and a MIME type. Past this many the index starts over
/// rather than tracking which were used last.
const MAX_INDEXED_FILES: usize = 4096;

impl MediaIndex {
    /// Path and MIME type of `file`, from the index or else the store.
    pub(crate) fn locate(
        &self,
        file: &str,
        store: &StoreSource,
    ) -> Result<(PathBuf, String), StatusCode> {
        let known = self.files.read().unwrap().get(file).cloned();
        if let Some(found) = known.filter(|(path, _)| path.is_file()) {
            return Ok(found);
        }
        let found = lookup(&*store()?, file)?;
        let mut files = self.files.write().unwrap();
        if files.len() >= MAX_INDEXED_FILES {
            files.clear();
        }
        files.insert(file.to_owned(), found.clone());
        Ok(found)
    }
}

pub(crate) fn lookup(
    store: &Store,
    file: &str,
) -> Result<(std::path::PathBuf, String), StatusCode> {
    let (id, ext) = parse_media_file(file).ok_or(StatusCode::NOT_FOUND)?;
    let image = store
        .get_image(id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if super::files::mime_for_extension(ext) != Some(image.mime_type.as_str()) {
        return Err(StatusCode::NOT_FOUND);
    }
    let path = crate::store::images::image_file_path(&image.project_id, id, &image.filename)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((path, image.mime_type))
}

/// Inclusive byte range. Malformed, multiple, empty and unsatisfiable ranges
/// return None; callers respond with 416 and the resource's complete length.
pub(crate) fn range_slice(len: u64, header: &str) -> Option<(u64, u64)> {
    let value = header.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    let number = |s: &str| -> Option<u64> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    if len == 0 {
        return None;
    }
    if start.is_empty() {
        let suffix = number(end)?;
        return (suffix > 0).then_some((len.saturating_sub(suffix), len - 1));
    }
    let start = number(start)?;
    let end = if end.is_empty() {
        len - 1
    } else {
        number(end)?.min(len - 1)
    };
    (start <= end && start < len).then_some((start, end))
}

pub(crate) fn response(
    index: &MediaIndex,
    store: &StoreSource,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    match serve(index, store, &request) {
        Ok(response) => response,
        Err(status) => Response::builder()
            .status(status)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Vec::new())
            .unwrap(),
    }
}

fn serve(
    index: &MediaIndex,
    store: &StoreSource,
    request: &Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>, StatusCode> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    let file_name = request
        .uri()
        .path()
        .strip_prefix('/')
        .ok_or(StatusCode::NOT_FOUND)?;
    let (path, mime) = index.locate(file_name, store)?;
    let mut file = std::fs::File::open(path).map_err(|_| StatusCode::NOT_FOUND)?;
    let len = file
        .metadata()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .len();
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header("X-Content-Type-Options", "nosniff");
    let (start, count) =
        if request.method() == Method::GET && request.headers().contains_key(header::RANGE) {
            let range = request
                .headers()
                .get(header::RANGE)
                .and_then(|h| h.to_str().ok())
                .and_then(|h| range_slice(len, h));
            let Some((start, end)) = range else {
                return Ok(builder
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                    .body(Vec::new())
                    .unwrap());
            };
            builder = builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
            (start, end - start + 1)
        } else {
            (0, len)
        };
    let mut bytes = Vec::new();
    if request.method() != Method::HEAD {
        file.seek(SeekFrom::Start(start))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        file.take(count)
            .read_to_end(&mut bytes)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if bytes.len() as u64 != count {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    Ok(builder
        .header(header::CONTENT_LENGTH, count)
        .body(bytes)
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_browser_seeking_and_invalid_requests() {
        for (header, expected) in [
            ("bytes=0-0", Some((0, 0))),
            ("bytes=3-", Some((3, 9))),
            ("bytes=-3", Some((7, 9))),
            ("bytes=-99", Some((0, 9))),
            ("bytes=8-99", Some((8, 9))),
            ("bytes=10-", None),
            ("bytes=5-4", None),
            ("bytes=-0", None),
            ("bytes=0-1,4-5", None),
            ("items=0-1", None),
            ("bytes=+1-2", None),
            ("bytes=-", None),
        ] {
            assert_eq!(range_slice(10, header), expected, "{header}");
        }
        assert_eq!(range_slice(0, "bytes=0-"), None);
    }

    /// The index holds no more than `MAX_INDEXED_FILES` entries. One past
    /// that it starts over: what it forgot is found through the store again,
    /// at the cost of one lookup each.
    #[test]
    fn the_index_starts_over_at_its_cap_and_finds_forgotten_files_again() {
        use crate::note_media::tests::Fixture;
        use std::sync::atomic::Ordering;
        let fixture = Fixture::new();
        let (first, _) = fixture.video(64);
        let (second, _) = fixture.video(128);
        assert_ne!(first, second);
        let index = MediaIndex::default();
        let (store, asked) = fixture.store_source();
        let entries = || index.files.read().unwrap().len();

        let first_found = index.locate(&first, &*store).unwrap();
        assert_eq!((entries(), asked.load(Ordering::SeqCst)), (1, 1));

        // One short of the cap: nothing is forgotten yet.
        {
            let mut files = index.files.write().unwrap();
            for n in files.len()..MAX_INDEXED_FILES - 1 {
                let entry = (PathBuf::from(format!("/nowhere/{n}")), String::new());
                files.insert(format!("filler-{n}"), entry);
            }
        }
        let second_found = index.locate(&second, &*store).unwrap();
        assert_eq!(entries(), MAX_INDEXED_FILES);
        assert_eq!(index.locate(&first, &*store).unwrap(), first_found);
        assert_eq!(asked.load(Ordering::SeqCst), 2);

        // At the cap, the next new file empties it and goes in alone.
        index
            .files
            .write()
            .unwrap()
            .remove(&second)
            .expect("the second video was indexed");
        index.files.write().unwrap().insert(
            "filler-last".into(),
            (PathBuf::from("/nowhere"), String::new()),
        );
        assert_eq!(entries(), MAX_INDEXED_FILES);
        assert_eq!(index.locate(&second, &*store).unwrap(), second_found);
        assert_eq!((entries(), asked.load(Ordering::SeqCst)), (1, 3));

        // The first video was forgotten with the rest: one more lookup finds
        // it where it was, and after that it is indexed again.
        assert_eq!(index.locate(&first, &*store).unwrap(), first_found);
        assert_eq!((entries(), asked.load(Ordering::SeqCst)), (2, 4));
        assert_eq!(index.locate(&first, &*store).unwrap(), first_found);
        assert_eq!(index.locate(&second, &*store).unwrap(), second_found);
        assert_eq!((entries(), asked.load(Ordering::SeqCst)), (2, 4));
    }
}
