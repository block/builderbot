//! Inline markdown image destinations. Keep byte ranges so captions and titles
//! round-trip verbatim. Reference-style images are deliberately not ingested.

use std::ops::Range;

#[derive(Debug)]
pub(crate) struct MediaRef {
    pub span: Range<usize>,
    pub destination: Range<usize>,
    pub target: String,
    pub alt: String,
}

pub(crate) fn extract_media_refs(markdown: &str) -> Vec<MediaRef> {
    let mut refs = Vec::new();
    for range in super::blocks::prose_ranges(markdown) {
        extract_inline(&markdown[range.clone()], range.start, &mut refs);
    }
    refs
}

fn extract_inline(line: &str, offset: usize, refs: &mut Vec<MediaRef>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            let length = bytes[i..].iter().take_while(|b| **b == b'`').count();
            let mut end = i + length;
            while end < bytes.len() {
                if bytes[end] == b'`' {
                    let closing = bytes[end..].iter().take_while(|b| **b == b'`').count();
                    if closing == length {
                        break;
                    }
                    end += closing;
                } else {
                    end += 1;
                }
            }
            i = if end < bytes.len() {
                end + length
            } else {
                i + length
            };
            continue;
        }
        if bytes[i..].starts_with(b"![") {
            if let Some((end, destination, alt)) = parse_image(bytes, i) {
                refs.push(MediaRef {
                    span: offset + i..offset + end,
                    alt: line[alt].to_owned(),
                    target: unescape_destination(&line[destination.clone()]),
                    destination: offset + destination.start..offset + destination.end,
                });
                i = end;
                continue;
            }
        }
        i += 1;
    }
}

fn parse_image(bytes: &[u8], start: usize) -> Option<(usize, Range<usize>, Range<usize>)> {
    let mut i = start + 2;
    let mut depth = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if bytes.get(i + 1) != Some(&b'(') {
        return None;
    }
    let alt = start + 2..i;
    i += 2;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let angle = bytes.get(i) == Some(&b'<');
    if angle {
        i += 1;
    }
    let dest_start = i;
    depth = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'\\' {
            i += 2;
            continue;
        }
        if angle {
            if byte == b'>' {
                break;
            }
            if byte == b'\n' || byte == b'<' {
                return None;
            }
        } else {
            match byte {
                b'(' => depth += 1,
                b')' if depth > 0 => depth -= 1,
                b')' => break,
                b if b.is_ascii_whitespace() => break,
                _ => {}
            }
        }
        i += 1;
    }
    let destination = dest_start..i;
    if destination.is_empty() {
        return None;
    }
    if angle {
        if bytes.get(i) != Some(&b'>') {
            return None;
        }
        i += 1;
    }
    let before_space = i;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if i > before_space && matches!(bytes.get(i), Some(b'"' | b'\'' | b'(')) {
        let quote = if bytes[i] == b'(' { b')' } else { bytes[i] };
        i += 1;
        while bytes.get(i).is_some_and(|b| *b != quote) {
            i += if bytes[i] == b'\\' { 2 } else { 1 };
        }
        if bytes.get(i) != Some(&quote) {
            return None;
        }
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
    }
    (bytes.get(i) == Some(&b')')).then_some((i + 1, destination, alt))
}

fn unescape_destination(value: &str) -> String {
    let mut result = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
            result.push(chars.next().unwrap());
        } else {
            result.push(ch);
        }
    }
    result
}

pub(crate) fn parse_media_file(file: &str) -> Option<(&str, &str)> {
    let (id, ext) = file.rsplit_once('.')?;
    let uuid = uuid::Uuid::parse_str(id).ok()?;
    (uuid.to_string() == id && super::files::mime_for_extension(ext).is_some()).then_some((id, ext))
}

pub(crate) fn stored_ref(target: &str) -> Option<(&str, &str)> {
    parse_media_file(target.strip_prefix("staged-media://")?)
}

pub(crate) fn media_ids(content: &str) -> std::collections::HashSet<String> {
    extract_media_refs(content)
        .iter()
        .filter_map(|r| stored_ref(&r.target).map(|(id, _)| id.to_owned()))
        .collect()
}

/// Error labels are plain text, even when the filename contains markdown/HTML.
pub(crate) fn unavailable(target: &str, reason: &str) -> String {
    let name = target.rsplit('/').next().unwrap_or("media");
    let escaped = escape_label(name);
    format!("{escaped} ({reason})")
}

pub(crate) fn escape_label(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if c.is_ascii_punctuation() {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}
