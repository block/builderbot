//! ID-only media lookup and seek-based responses for the desktop URI protocol.

use super::refs::parse_media_file;
use crate::store::Store;
use std::io::{Read, Seek, SeekFrom};
use tauri::http::{header, Method, Request, Response, StatusCode};

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

pub(crate) fn response(store: &Store, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    match serve(store, &request) {
        Ok(response) => response,
        Err(status) => Response::builder()
            .status(status)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Vec::new())
            .unwrap(),
    }
}

fn serve(store: &Store, request: &Request<Vec<u8>>) -> Result<Response<Vec<u8>>, StatusCode> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    let file_name = request
        .uri()
        .path()
        .strip_prefix('/')
        .ok_or(StatusCode::NOT_FOUND)?;
    let (path, mime) = lookup(store, file_name)?;
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
}
