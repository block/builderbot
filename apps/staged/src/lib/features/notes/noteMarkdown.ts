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
 *
 * The title goes back exactly as stored, escapes and all — which is to say
 * without any. A stored title holding a *complete* inline construct is therefore
 * re-read as markup here: `a _b_ c` reopens italic, and the next save stores the
 * equivalent `a *b* c`, stable from there. Re-escaping instead would push
 * backslashes into the markdown the viewer renders and the user copies, to defend
 * a case that needs a plain-text paste to reach. Unpaired punctuation — the `_`
 * of an identifier, an `&` — cannot re-form markup and round-trips exactly.
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

/** The ASCII punctuation CommonMark lets a backslash escape — those 32 and no more. */
const MARKDOWN_ESCAPE = /\\([!-/:-@[-`{-~])/g;

/**
 * The text a reader sees for one line of serialized markdown: `snake\_case\_name`
 * reads as `snake_case_name`.
 *
 * The editor's serializer escapes any punctuation that would otherwise be markup,
 * so a line coming back out of it is spelled for the parser rather than for a
 * person. Undoing that is a pair rule, not a hunt for backslashes: the escaped
 * character is consumed along with its backslash, so `\\` leaves the one literal
 * backslash the user typed, and a backslash before anything not on the list — a
 * letter, an em dash — is itself literal and stays.
 */
export function unescapeMarkdown(line: string): string {
  return line.replace(MARKDOWN_ESCAPE, '$1');
}

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
 * Whether a line of serialized markdown can stand in as the note's title.
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
 * The escapes come off before the rule runs, so a typed-out URL is judged as the
 * `https://…` a reader sees rather than the `https\://…` the serializer wrote.
 *
 * The editor applies the same rule live, so what looks like a title on screen
 * is what gets stored (see `wysiwygPlugins`).
 */
export function canBeNoteTitleLine(line: string): boolean {
  return canBeNoteTitleText(unescapeMarkdown(line));
}

/**
 * [`canBeNoteTitleLine`] for text that is already plain.
 *
 * The editor holds parsed nodes, so a block's `textContent` never carries the
 * serializer's escapes; unescaping it a second time would let the editor and the
 * save path disagree about the same block. A paragraph whose visible text is
 * `\- item` is the case: a second pass reads that as the bullet `- item` and
 * refuses it, while the save path unescapes the serialized `\\- item` once and
 * takes `\- item` — plain text, which is what it is — as the title.
 */
export function canBeNoteTitleText(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed) return false;
  return !NON_TITLE_BLOCK.test(trimmed) && !LINK_OR_IMAGE.test(trimmed);
}

/** A leading ATX heading marker, up to the space that ends it. */
const HEADING_MARKER = /^(#{1,6})[ \t]+/;

/**
 * Split a note's markdown into the `(title, body)` pair the store keeps.
 *
 * A note is a title column plus a body with that title line already taken out of
 * it — the shape `resolve_note_title_and_body` produces for session notes, so
 * viewers and `#note:` references treat every note alike. `noteMarkdownWithTitle`
 * is the exact inverse and puts the line back unconditionally, so every writer
 * has to hand over a body the title has left. This is where that happens, for
 * the editor and for a dropped file alike.
 *
 * `fallbackTitle` is a name the caller already has for the note — the file name,
 * on the drop path. A caller holding one gives it up only to a leading `# H1`:
 * the document naming itself, which is both the better title and the line that
 * would otherwise be shown directly under it. Anything else on line one is
 * content there — a log's first line is not its title — and stays in the body.
 *
 * The editor has no such name, since it has no title field: there the first
 * line is the title, heading or not, and it leaves the body either way. (The
 * editor shows this by promoting that line to an H1 as it's typed; see
 * `wysiwygPlugins`.)
 *
 * Either way, a first line that can't be a title ([`canBeNoteTitleLine`]) is not
 * consumed: it is content, so it stays in the body and the note keeps the
 * fallback name, or is Untitled without one. Reopening then puts a real title
 * line above it, and the next save reads that instead — no line is lost or
 * duplicated on the way through.
 *
 * The title crosses out of markdown here, so this is where the serializer's
 * escapes come off it: the column holds the text a reader sees.
 */
export function splitNoteMarkdown(
  markdown: string,
  fallbackTitle = ''
): { title: string; body: string } {
  const named = fallbackTitle.trim();
  const untitled = named || UNTITLED_NOTE_TITLE;

  const lines = markdown.split('\n');
  const titleIndex = lines.findIndex((line) => line.trim().length > 0);
  if (titleIndex === -1) return { title: untitled, body: '' };

  const bodyFrom = (index: number) => lines.slice(index).join('\n').trimStart();
  const firstLine = lines[titleIndex].trim();
  // Read for structure on the raw line: only a genuine leading `# ` is the
  // document naming itself. `\# Heading` is a paragraph a reader sees as
  // `# Heading`, and unescaping it first would take that `#` for a marker.
  const isDocumentTitle = firstLine.match(HEADING_MARKER)?.[1] === '#';
  if (!canBeNoteTitleLine(firstLine) || (named && !isDocumentTitle)) {
    return { title: untitled, body: bodyFrom(titleIndex) };
  }

  const title = unescapeMarkdown(firstLine.replace(HEADING_MARKER, '')).trim();
  return { title: title || untitled, body: bodyFrom(titleIndex + 1) };
}

export function renderNoteMarkdown(text: string, options: MarkdownRenderingOptions = {}): string {
  return renderMarkdown(text, options);
}
