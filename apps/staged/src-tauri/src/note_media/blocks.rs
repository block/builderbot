//! Markdown block structure for media ingestion: which lines render as prose
//! rather than fenced or indented code or an HTML block. Follows the
//! CommonMark container rules Marked uses, so indentation is judged relative
//! to the enclosing list item or blockquote. A misread code example would have
//! its image rewritten, so this errs towards code only where the renderer does.

use crate::pikchr_validation::{is_closing_fence, parse_opening_fence};
use std::ops::Range;

/// Byte ranges of note text that Markdown renders as prose (not fenced or
/// indented code, nor an HTML block). Consecutive prose lines are merged so
/// inline syntax that spans lines is preserved.
pub(crate) fn prose_ranges(markdown: &str) -> Vec<Range<usize>> {
    let mut scanner = Scanner::default();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut offset = 0;
    for line in markdown.split_inclusive('\n') {
        let end = offset + line.len();
        if scanner.is_prose(line.trim_end_matches(['\r', '\n'])) {
            match ranges.last_mut() {
                Some(last) if last.end == offset => last.end = end,
                _ => ranges.push(offset..end),
            }
        }
        offset = end;
    }
    ranges
}

enum Container {
    Quote,
    /// Content indent in columns, relative to the parent container's content.
    Item {
        indent: usize,
    },
}

/// How an open HTML block ends (CommonMark 4.6 start conditions 1, 2, and 6).
/// Condition 7, an arbitrary complete tag on its own line, is not recognized.
#[derive(Clone, Copy)]
enum HtmlBlock {
    /// `<pre>`, `<script>`, `<style>`, `<textarea>`: ends on a line containing
    /// the closing tag of the tag that opened it, blank lines included. Marked
    /// matches the closer with a backreference, so `</script>` does not end a
    /// `<pre>` block, and a block with no matching closer runs to the end of
    /// the document.
    Raw(RawTag),
    /// `<!--`: ends on a line containing `-->`.
    Comment,
    /// A block-level tag: ends at the next blank line.
    Block,
}

/// The tags of CommonMark 4.6 start condition 1, whose content is raw.
#[derive(Clone, Copy)]
enum RawTag {
    Pre,
    Script,
    Style,
    Textarea,
}

impl RawTag {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "pre" => Some(RawTag::Pre),
            "script" => Some(RawTag::Script),
            "style" => Some(RawTag::Style),
            "textarea" => Some(RawTag::Textarea),
            _ => None,
        }
    }

    fn closer(self) -> &'static str {
        match self {
            RawTag::Pre => "</pre>",
            RawTag::Script => "</script>",
            RawTag::Style => "</style>",
            RawTag::Textarea => "</textarea>",
        }
    }
}

impl HtmlBlock {
    /// Whether this line meets the end condition. The start line may.
    fn ends_on(self, content: &str) -> bool {
        match self {
            HtmlBlock::Raw(tag) => content.to_ascii_lowercase().contains(tag.closer()),
            HtmlBlock::Comment => content.contains("-->"),
            HtmlBlock::Block => false,
        }
    }
}

#[derive(Default)]
struct Scanner {
    stack: Vec<Container>,
    fence: Option<(char, usize)>,
    html: Option<HtmlBlock>,
    in_paragraph: bool,
    /// Whether the previous line was blank; a dedented line after a blank
    /// ends a list item even inside its fence or HTML block.
    last_blank: bool,
}

impl Scanner {
    fn is_prose(&mut self, line: &str) -> bool {
        let prose = self.scan(line);
        self.last_blank = line.trim_matches([' ', '\t']).is_empty();
        prose
    }

    fn scan(&mut self, line: &str) -> bool {
        let mut cursor = Cursor::new(line);
        let matched = self.match_containers(&mut cursor);
        if self.fence.is_some() || self.html.is_some() {
            if matched < self.stack.len() && self.leaves_container(&cursor, matched) {
                // The container closed, taking its fence or HTML block with
                // it. The line is then read afresh at the outer level, where a
                // dedented ``` opens a new fence rather than closing one.
                self.stack.truncate(matched);
                self.fence = None;
                self.html = None;
                self.in_paragraph = false;
            } else if let Some((character, length)) = self.fence {
                // Marked's list items absorb other dedented lines, so prefixes
                // are only stripped (leniently) to find the closing fence.
                if cursor.indent() < 4 && is_closing_fence(cursor.content(), character, length) {
                    self.fence = None;
                }
                return false;
            } else if let Some(html) = self.html {
                match html {
                    HtmlBlock::Block if cursor.is_blank() => self.html = None,
                    _ => {
                        if html.ends_on(cursor.content()) {
                            self.html = None;
                        }
                        return false;
                    }
                }
            }
        }
        if cursor.is_blank() {
            self.stack.truncate(matched);
            self.in_paragraph = false;
            return true;
        }
        if matched < self.stack.len() {
            if self.in_paragraph && !starts_block(&cursor) {
                return true;
            }
            self.stack.truncate(matched);
            self.in_paragraph = false;
        }
        self.open_blocks(&mut cursor)
    }

    /// Consumes the prefixes of open containers, returning how many matched.
    /// Blank lines never close list items.
    fn match_containers(&self, cursor: &mut Cursor) -> usize {
        for (depth, container) in self.stack.iter().enumerate() {
            let matched = match *container {
                Container::Quote => cursor.take_quote(),
                Container::Item { indent } => {
                    let fits = cursor.indent() >= indent || cursor.is_blank();
                    if fits {
                        cursor.skip_columns(indent);
                    }
                    fits
                }
            };
            if !matched {
                return depth;
            }
        }
        self.stack.len()
    }

    /// Whether a line that failed to match the container at `depth` ends it
    /// while a fence or HTML block is open. Blockquotes end as CommonMark
    /// says; Marked's list items also absorb dedented lines unless a blank
    /// line preceded them or they would start a block.
    fn leaves_container(&self, cursor: &Cursor, depth: usize) -> bool {
        match self.stack[depth] {
            Container::Quote => true,
            Container::Item { .. } => self.last_blank || starts_block(cursor),
        }
    }

    fn open_blocks(&mut self, cursor: &mut Cursor) -> bool {
        loop {
            if cursor.is_blank() {
                return true;
            }
            if cursor.indent() >= 4 {
                // Indented code cannot interrupt a paragraph.
                return self.in_paragraph;
            }
            if cursor.take_quote() {
                self.stack.push(Container::Quote);
                self.in_paragraph = false;
                continue;
            }
            let content = cursor.content();
            if (self.in_paragraph && is_setext_underline(content))
                || is_thematic_break(content)
                || is_atx_heading(content)
            {
                self.in_paragraph = false;
                return true;
            }
            if let Some(indent) = cursor.take_list_marker() {
                self.stack.push(Container::Item { indent });
                self.in_paragraph = false;
                continue;
            }
            if let Some(opening) = parse_opening_fence(content) {
                self.fence = Some((opening.fence_char, opening.fence_length));
                self.in_paragraph = false;
                return false;
            }
            if let Some(html) = html_block_start(content) {
                // A start line that also meets the end condition is the whole
                // block.
                if !html.ends_on(content) {
                    self.html = Some(html);
                }
                self.in_paragraph = false;
                return false;
            }
            self.in_paragraph = true;
            return true;
        }
    }
}

/// Whether a line that failed to match its containers starts a new block
/// instead of lazily continuing the open paragraph.
fn starts_block(cursor: &Cursor) -> bool {
    let content = cursor.content();
    cursor.indent() < 4
        && (content.starts_with('>')
            || list_marker_width(content).is_some()
            || parse_opening_fence(content).is_some()
            || is_atx_heading(content)
            || is_thematic_break(content)
            || html_block_start(content).is_some())
}

/// Tags whose open or close tag on a line starts an HTML block that runs to
/// the next blank line (CommonMark 4.6, condition 6).
const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

/// HTML block start conditions 1, 2, and 6 for a line's content (after
/// container prefixes and under four columns of indentation). All three may
/// interrupt a paragraph.
fn html_block_start(content: &str) -> Option<HtmlBlock> {
    let rest = content.strip_prefix('<')?;
    if rest.starts_with("!--") {
        return Some(HtmlBlock::Comment);
    }
    let (closing, rest) = match rest.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let name_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    if name_len == 0 {
        return None;
    }
    let name = rest[..name_len].to_ascii_lowercase();
    let after = &rest[name_len..];
    let delimited = matches!(after.as_bytes().first(), None | Some(b' ' | b'\t' | b'>'));
    if !closing && delimited {
        if let Some(tag) = RawTag::from_name(&name) {
            return Some(HtmlBlock::Raw(tag));
        }
    }
    if (delimited || after.starts_with("/>")) && BLOCK_TAGS.contains(&name.as_str()) {
        return Some(HtmlBlock::Block);
    }
    None
}

/// A position within one line, in columns with tabs expanded to the next
/// multiple of 4. Container prefixes may consume part of a tab.
struct Cursor<'a> {
    text: &'a str,
    pos: usize,
    col: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            pos: 0,
            col: 0,
        }
    }

    /// The remaining text after leading whitespace.
    fn content(&self) -> &'a str {
        self.text[self.pos..].trim_start_matches([' ', '\t'])
    }

    fn is_blank(&self) -> bool {
        self.content().is_empty()
    }

    /// Columns of whitespace before the next non-whitespace character.
    fn indent(&self) -> usize {
        let mut col = self.col;
        for byte in self.text[self.pos..].bytes() {
            match byte {
                b' ' => col += 1,
                b'\t' => col += 4 - col % 4,
                _ => break,
            }
        }
        col - self.col
    }

    /// Consumes up to `columns` of whitespace, splitting a tab if needed.
    fn skip_columns(&mut self, mut columns: usize) {
        while columns > 0 {
            let width = match self.text.as_bytes().get(self.pos) {
                Some(b' ') => 1,
                Some(b'\t') => 4 - self.col % 4,
                _ => return,
            };
            let step = width.min(columns);
            self.col += step;
            columns -= step;
            if step == width {
                self.pos += 1;
            }
        }
    }

    /// Consumes an ASCII marker after skipping indentation of under 4 columns.
    fn take_marker(&mut self, width: usize) {
        self.skip_columns(self.indent());
        self.pos += width;
        self.col += width;
    }

    /// `>` with up to three columns of indentation and one optional space.
    fn take_quote(&mut self) -> bool {
        if self.indent() >= 4 || !self.content().starts_with('>') {
            return false;
        }
        self.take_marker(1);
        self.skip_columns(1);
        true
    }

    /// Opens a list item, returning its content indent relative to the
    /// current column: 1–4 spaces after the marker count towards it, while
    /// 5 or more (or none before the line ends) count as one so the rest is
    /// code.
    fn take_list_marker(&mut self) -> Option<usize> {
        if self.indent() >= 4 {
            return None;
        }
        let width = list_marker_width(self.content())?;
        let start = self.col;
        self.take_marker(width);
        let spaces = self.indent();
        let padding = if self.is_blank() || spaces > 4 {
            1
        } else {
            spaces
        };
        let indent = self.col - start + padding;
        self.skip_columns(padding);
        Some(indent)
    }
}

fn list_marker_width(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let width = match bytes.first()? {
        b'-' | b'*' | b'+' => 1,
        _ => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if !(1..=9).contains(&digits) || !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return None;
            }
            digits + 1
        }
    };
    matches!(bytes.get(width), None | Some(b' ' | b'\t')).then_some(width)
}

fn is_atx_heading(text: &str) -> bool {
    let hashes = text.bytes().take_while(|b| *b == b'#').count();
    (1..=6).contains(&hashes) && matches!(text.as_bytes().get(hashes), None | Some(b' ' | b'\t'))
}

fn is_thematic_break(text: &str) -> bool {
    let mut chars = text.chars().filter(|c| !matches!(c, ' ' | '\t'));
    let Some(first @ ('-' | '*' | '_')) = chars.next() else {
        return false;
    };
    let mut count = 1;
    for c in chars {
        if c != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}

fn is_setext_underline(text: &str) -> bool {
    let text = text.trim_end_matches([' ', '\t']);
    !text.is_empty() && (text.bytes().all(|b| b == b'=') || text.bytes().all(|b| b == b'-'))
}
