import {
  renderMarkdown,
  type MarkdownRenderingOptions,
} from '../../shared/markdown/renderMarkdown';

export function noteMarkdownWithTitle(title: string, content: string): string {
  const normalizedTitle = title.trim();
  if (!normalizedTitle) return content;

  const normalizedContent = content.trimStart();
  if (!normalizedContent) return `# ${normalizedTitle}`;
  if (startsWithTitleLine(normalizedContent, normalizedTitle)) return content;

  return `# ${normalizedTitle}\n\n${normalizedContent}`;
}

/**
 * True when the content already opens with its own title line, so prepending
 * one would show the user a duplicate.
 *
 * A leading H1 is the stored shape and wins outright, whatever the title
 * column says. Otherwise the test is whether `splitNoteMarkdown` would read
 * this very title back off the content: its no-H1 fallback keeps the whole
 * document as the body, title line included.
 */
function startsWithTitleLine(content: string, title: string): boolean {
  return startsWithMarkdownH1(content) || titleFromFirstLine(content) === title;
}

function startsWithMarkdownH1(content: string): boolean {
  return /^#[ \t]+\S/.test(content);
}

/** Title used when a written note has no heading and no usable first line. */
export const UNTITLED_NOTE_TITLE = 'Untitled note';

const TITLE_MAX_LENGTH = 80;

/**
 * Split a written note's markdown into the `(title, body)` pair the store keeps.
 *
 * Notes have no separate title field: the leading H1 is the title, and the
 * stored content is everything after it — the same shape `resolve_note_title_and_body`
 * produces for session notes, so viewers and `#note:` references treat both alike.
 * `noteMarkdownWithTitle` is the inverse, recombining them for display or editing.
 *
 * Without a leading H1 there is no session prompt to fall back on, so the first
 * non-empty line stands in, clipped to a title-sized string. The body then keeps
 * that line — it is real content, a list item or a demoted heading — which is why
 * `noteMarkdownWithTitle` recognises it instead of prepending a second copy.
 */
export function splitNoteMarkdown(markdown: string): { title: string; body: string } {
  const trimmed = markdown.trimStart();
  const h1 = /^#[ \t]+(.*)/.exec(trimmed);
  if (h1) {
    const newline = trimmed.indexOf('\n');
    const title = (
      newline === -1 ? h1[1] : trimmed.slice(0, newline).replace(/^#[ \t]+/, '')
    ).trim();
    const body = newline === -1 ? '' : trimmed.slice(newline + 1).trimStart();
    if (title) return { title, body };
  }
  return { title: titleFromFirstLine(markdown), body: markdown };
}

function titleFromFirstLine(markdown: string): string {
  const firstLine = markdown
    .split('\n')
    .map((line) => line.replace(/^#{1,6}[ \t]+/, '').trim())
    .find((line) => line.length > 0);
  if (!firstLine) return UNTITLED_NOTE_TITLE;
  return firstLine.length > TITLE_MAX_LENGTH
    ? `${firstLine.slice(0, TITLE_MAX_LENGTH)}…`
    : firstLine;
}

export function renderNoteMarkdown(text: string, options: MarkdownRenderingOptions = {}): string {
  return renderMarkdown(text, options);
}
