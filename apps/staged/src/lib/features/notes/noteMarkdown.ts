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
 * Split a written note's markdown into the `(title, body)` pair the store keeps.
 *
 * Notes have no separate title field in the editor: the first line is the title
 * and the body is everything after it — the same shape `resolve_note_title_and_body`
 * produces for session notes, so viewers and `#note:` references treat both alike.
 * `noteMarkdownWithTitle` is the exact inverse, so an edit round-trips whatever
 * follows, including a body that opens with a heading of its own.
 *
 * Heading markers come off the title: it is stored as text and rendered as the
 * note's H1, so `#`s would otherwise show up literally in the timeline. Nothing
 * else about the line is interpreted — the editor promotes the first line to an
 * H1 as it is typed (`wysiwygPlugins`), so this normally reads a real title back,
 * and a document pasted with a list or a quote on line one has that line taken
 * as the title as written rather than left to duplicate itself in the body.
 */
export function splitNoteMarkdown(markdown: string): { title: string; body: string } {
  const lines = markdown.split('\n');
  const titleIndex = lines.findIndex((line) => line.trim().length > 0);
  if (titleIndex === -1) return { title: UNTITLED_NOTE_TITLE, body: '' };

  const title = lines[titleIndex].replace(/^#{1,6}[ \t]+/, '').trim();
  const body = lines
    .slice(titleIndex + 1)
    .join('\n')
    .trimStart();
  return { title: title || UNTITLED_NOTE_TITLE, body };
}

export function renderNoteMarkdown(text: string, options: MarkdownRenderingOptions = {}): string {
  return renderMarkdown(text, options);
}
