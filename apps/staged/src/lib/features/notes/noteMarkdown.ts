import {
  renderMarkdown,
  type MarkdownRenderingOptions,
} from '../../shared/markdown/renderMarkdown';

/**
 * Recombine a stored note's `(title, content)` pair into displayable markdown.
 *
 * The title has its own column and the content is the body with the title line
 * already taken out of it, so the H1 goes back on unconditionally. Skipping it
 * when the body happens to open with a heading of its own would hide the real
 * title — and in the editor the next save would then read that heading as the
 * title and overwrite the stored one.
 *
 * An empty title means a session stub the runner titles later; there is nothing
 * to prepend yet.
 */
export function noteMarkdownWithTitle(title: string, content: string): string {
  const normalizedTitle = title.trim();
  if (!normalizedTitle) return content;

  const normalizedContent = content.trimStart();
  if (!normalizedContent) return `# ${normalizedTitle}`;

  return `# ${normalizedTitle}\n\n${normalizedContent}`;
}

/** Title used when a written note has no usable first line. */
export const UNTITLED_NOTE_TITLE = 'Untitled note';

/**
 * Markdown that only means something as a block: a list item, a quote, a fence,
 * a table row, a rule, raw HTML. Stripped of its context it is punctuation.
 */
const NON_TITLE_BLOCK =
  /^(?:[-*+](?:[ \t]|$)|\d{1,9}[.)](?:[ \t]|$)|>|`{3,}|~{3,}|\||<|(?:[-*_][ \t]*){3,}$)/;

/** An image, an inline or reference link, an autolink, or a bare URL. */
const LINK_OR_IMAGE =
  /!?\[[^\]\n]*\](?:\([^)\n]*\)|\[[^\]\n]*\])|<[a-z][a-z\d+.-]*:[^\s>]*>|\b(?:https?:\/\/|www\.)\S/i;

/**
 * Whether a line can stand in as the note's title.
 *
 * The title is stored as plain text and shown as one line in the timeline, so
 * anything whose meaning lives in its markup reads there as raw syntax — a
 * bullet's `-`, a link's `[…](…)`, an image that has no text at all. Those lines
 * stay in the body and the note is titled [`UNTITLED_NOTE_TITLE`] instead.
 *
 * Headings are the exception: `#` markers are the title's own syntax and come
 * off in [`splitNoteMarkdown`]. Emphasis and inline code are left alone — they
 * are decoration on text that still reads as a title.
 *
 * The editor applies the same rule live, so what looks like a title on screen
 * is what gets stored (see `wysiwygPlugins`).
 */
export function canBeNoteTitle(line: string): boolean {
  // Backslashes come off first: the editor's serializer escapes punctuation that
  // would otherwise be markup, so a typed-out URL reaches this as `https\://…`.
  // What matters is the line the reader ends up seeing, not how it is spelled.
  const trimmed = line.trim().replace(/\\(?=[^\p{L}\p{N}\s])/gu, '');
  if (!trimmed) return false;
  return !NON_TITLE_BLOCK.test(trimmed) && !LINK_OR_IMAGE.test(trimmed);
}

/**
 * Split a written note's markdown into the `(title, body)` pair the store keeps.
 *
 * Notes have no separate title field in the editor: the first line is the title
 * and the body is everything after it — the same shape `resolve_note_title_and_body`
 * produces for session notes, so viewers and `#note:` references treat both alike.
 * `noteMarkdownWithTitle` is the exact inverse, so an edit round-trips whatever
 * follows, including a body that opens with a heading of its own.
 *
 * A first line that can't be a title ([`canBeNoteTitle`]) is not consumed: it is
 * content, so it stays in the body and the note is Untitled. Reopening then puts
 * a real title line above it, and the next save reads that instead — no line is
 * lost or duplicated on the way through.
 */
export function splitNoteMarkdown(markdown: string): { title: string; body: string } {
  const lines = markdown.split('\n');
  const titleIndex = lines.findIndex((line) => line.trim().length > 0);
  if (titleIndex === -1) return { title: UNTITLED_NOTE_TITLE, body: '' };

  const bodyFrom = (index: number) => lines.slice(index).join('\n').trimStart();
  const firstLine = lines[titleIndex];
  if (!canBeNoteTitle(firstLine)) {
    return { title: UNTITLED_NOTE_TITLE, body: bodyFrom(titleIndex) };
  }

  const title = firstLine.replace(/^#{1,6}[ \t]+/, '').trim();
  return { title: title || UNTITLED_NOTE_TITLE, body: bodyFrom(titleIndex + 1) };
}

export function renderNoteMarkdown(text: string, options: MarkdownRenderingOptions = {}): string {
  return renderMarkdown(text, options);
}
