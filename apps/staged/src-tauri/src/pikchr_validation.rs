use std::ffi::{CStr, CString};

use libc::{c_int, c_void, free};
use pikchr::PikchrFlags;

const PIKCHR_SVG_CLASS: &str = "markdown-pikchr-svg";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PikchrBlock {
    pub(crate) source: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PikchrValidationError {
    pub(crate) line_number: usize,
    pub(crate) message: String,
}

pub(crate) fn extract_pikchr_blocks(markdown: &str) -> Vec<PikchrBlock> {
    let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut blocks = Vec::new();
    let mut line_index = 0;

    while line_index < lines.len() {
        let Some(opening) = parse_opening_fence(lines[line_index]) else {
            line_index += 1;
            continue;
        };

        let content_start = line_index + 1;
        let closing_line_index = (content_start..lines.len()).find(|candidate| {
            is_closing_fence(lines[*candidate], opening.fence_char, opening.fence_length)
        });
        let content_end = closing_line_index.unwrap_or(lines.len());

        if is_pikchr_info_string(&opening.info_string) {
            blocks.push(PikchrBlock {
                source: lines[content_start..content_end].join("\n"),
                start_line: line_index + 1,
                end_line: closing_line_index.map(|closing| closing + 1),
            });
        }

        line_index = closing_line_index
            .map(|closing| closing + 1)
            .unwrap_or(lines.len());
    }

    blocks
}

pub(crate) fn validate_pikchr_blocks(markdown: &str) -> Vec<PikchrValidationError> {
    extract_pikchr_blocks(markdown)
        .into_iter()
        .filter_map(validate_pikchr_block)
        .collect()
}

fn validate_pikchr_block(block: PikchrBlock) -> Option<PikchrValidationError> {
    let error = render_pikchr_for_validation(&block.source).err()?;

    Some(PikchrValidationError {
        line_number: block.start_line,
        message: error,
    })
}

fn render_pikchr_for_validation(source: &str) -> Result<(), String> {
    let source = CString::new(source).map_err(|e| format!("{e:?}"))?;
    let class = CString::new(PIKCHR_SVG_CLASS).expect("static SVG class has no nul bytes");
    let mut width: c_int = 0;
    let mut height: c_int = 0;

    let rendered = unsafe {
        pikchr::raw::pikchr(
            source.as_ptr(),
            class.as_ptr(),
            PikchrFlags::default().into(),
            &mut width,
            &mut height,
        )
    };

    if rendered.is_null() {
        return Err("Pikchr returned no validation output.".to_string());
    }

    let output = unsafe { CStr::from_ptr(rendered) }
        .to_string_lossy()
        .trim()
        .to_string();
    unsafe {
        free(rendered as *mut c_void);
    }

    if width < 0 {
        Err(output)
    } else {
        Ok(())
    }
}

struct OpeningFence {
    fence_char: char,
    fence_length: usize,
    info_string: String,
}

fn parse_opening_fence(line: &str) -> Option<OpeningFence> {
    let rest = strip_allowed_indent(line)?;
    let fence_char = rest.chars().next()?;
    if fence_char != '`' && fence_char != '~' {
        return None;
    }

    let fence_length = rest.chars().take_while(|c| *c == fence_char).count();
    if fence_length < 3 {
        return None;
    }

    Some(OpeningFence {
        fence_char,
        fence_length,
        info_string: rest[fence_length..].trim().to_string(),
    })
}

fn is_closing_fence(line: &str, fence_char: char, fence_length: usize) -> bool {
    let Some(rest) = strip_allowed_indent(line) else {
        return false;
    };

    let closing_length = rest.chars().take_while(|c| *c == fence_char).count();
    if closing_length < fence_length {
        return false;
    }

    rest[closing_length..]
        .chars()
        .all(|c| c == ' ' || c == '\t')
}

fn strip_allowed_indent(line: &str) -> Option<&str> {
    let mut spaces = 0;
    for (idx, ch) in line.char_indices() {
        match ch {
            ' ' if spaces < 3 => spaces += 1,
            ' ' => return None,
            _ => return Some(&line[idx..]),
        }
    }
    Some("")
}

fn is_pikchr_info_string(info_string: &str) -> bool {
    info_string
        .split_whitespace()
        .next()
        .is_some_and(|language| language.eq_ignore_ascii_case("pikchr"))
}

#[cfg(test)]
mod tests {
    use super::{extract_pikchr_blocks, validate_pikchr_blocks};

    #[test]
    fn extracts_backtick_and_tilde_pikchr_fences_case_insensitively() {
        let blocks = extract_pikchr_blocks(
            r#"Before
```PIKCHR title="flow"
box "A" fit
```
~~~pikchr
box "B" fit
~~~
"#,
        );

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].start_line, 2);
        assert_eq!(blocks[0].end_line, Some(4));
        assert_eq!(blocks[0].source, "box \"A\" fit");
        assert_eq!(blocks[1].start_line, 5);
        assert_eq!(blocks[1].end_line, Some(7));
        assert_eq!(blocks[1].source, "box \"B\" fit");
    }

    #[test]
    fn ignores_non_pikchr_fences_while_skipping_their_contents() {
        let blocks = extract_pikchr_blocks(
            r#"```rust
let sample = "```pikchr";
```
```pikchr
box "Done" fit
```"#,
        );

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].start_line, 4);
        assert_eq!(blocks[0].source, "box \"Done\" fit");
    }

    #[test]
    fn tracks_unclosed_pikchr_fence_through_end_of_markdown() {
        let blocks = extract_pikchr_blocks("Before\n```pikchr\nbox \"Draft\" fit");

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].start_line, 2);
        assert_eq!(blocks[0].end_line, None);
        assert_eq!(blocks[0].source, "box \"Draft\" fit");
    }

    #[test]
    fn validates_valid_pikchr_blocks() {
        let errors = validate_pikchr_blocks("```pikchr\nbox \"Start\" fit\n```");
        assert!(errors.is_empty());
    }

    #[test]
    fn returns_line_number_and_parser_message_for_invalid_pikchr() {
        let errors = validate_pikchr_blocks("Intro\n```pikchr\nbox \"unterminated\n```");

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line_number, 2);
        assert!(!errors[0].message.is_empty());
    }
}
