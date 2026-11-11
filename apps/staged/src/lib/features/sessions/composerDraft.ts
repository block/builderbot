/**
 * A reply composer's draft: free text with quote blocks woven through it.
 *
 * Selecting text in a note and replying to it adds a quote block — the
 * selected text, the headings it sits under, and the user's reply — to the
 * note's chat draft. Several selections build up a review, and free text can
 * sit before, between and after the blocks.
 *
 * The draft is an ordered sequence `text, quote, text, quote, …, text`. Each
 * quote carries the free text typed *before* it, and `text` is the free text
 * after the last one (the composer's main input), so the alternation holds by
 * construction: there is no way to spell two quotes without a text gap
 * between them, or a draft that doesn't end in text.
 *
 * A draft with no quotes is just `text` and persists as the plain string it
 * always did (see `composerDraftContent`). On send the draft becomes one
 * markdown message (`serializeComposerDraft`), and the transcript reads the
 * quotes back out of it for display (`parseQuotedMessage`).
 */

import type { DraftContent } from '../../shared/draftCache';

export type DraftQuote = {
  /** Stable identity for keyed rendering and removal. */
  id: string;
  /** Free text typed before this quote (after the previous one). */
  before: string;
  /** The selected note text, as the reader saw it. */
  quote: string;
  /** Headings the selection sits under, outermost first. */
  headings: string[];
  /** The user's reply to this quote. */
  reply: string;
};

export type ComposerDraft = {
  quotes: DraftQuote[];
  /** Free text after the last quote — the composer's main input. */
  text: string;
};

/** What a selection contributes; the draft supplies `id` and `before`. */
export type NewDraftQuote = Pick<DraftQuote, 'quote' | 'headings' | 'reply'>;

/** Joins a quote's heading path, for display and in the sent message. */
export const HEADING_PATH_SEPARATOR = ' › ';

/** Version of the structured form persisted with a draft that holds quotes. */
const STORED_DRAFT_VERSION = 1;

type StoredComposerDraft = {
  version: typeof STORED_DRAFT_VERSION;
  quotes: DraftQuote[];
  text: string;
};

export function emptyComposerDraft(): ComposerDraft {
  return { quotes: [], text: '' };
}

/** Nothing to send: no quote and no text beyond whitespace. */
export function isComposerDraftEmpty(draft: ComposerDraft): boolean {
  return draft.quotes.length === 0 && draft.text.trim().length === 0;
}

/** Plain copies, safe to persist or compare (the pane's draft is a `$state` proxy). */
export function cloneComposerDraft(draft: ComposerDraft): ComposerDraft {
  return {
    quotes: draft.quotes.map((quote) => ({ ...quote, headings: [...quote.headings] })),
    text: draft.text,
  };
}

export function composerDraftsEqual(a: ComposerDraft, b: ComposerDraft): boolean {
  if (a.text !== b.text || a.quotes.length !== b.quotes.length) return false;
  return a.quotes.every((quote, index) => {
    const other = b.quotes[index];
    return (
      quote.id === other.id &&
      quote.before === other.before &&
      quote.quote === other.quote &&
      quote.reply === other.reply &&
      quote.headings.length === other.headings.length &&
      quote.headings.every((heading, i) => heading === other.headings[i])
    );
  });
}

export function newDraftQuoteId(): string {
  return (
    globalThis.crypto?.randomUUID?.() ??
    `quote-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
  );
}

/**
 * Add a quote at the end of the draft. Whatever was in the main input was
 * written before this quote, so it becomes the quote's `before` text and the
 * main input starts empty below the new block — the draft reads top to
 * bottom in the order it was built.
 */
export function appendDraftQuote(
  draft: ComposerDraft,
  quote: NewDraftQuote,
  id = newDraftQuoteId()
): ComposerDraft {
  return {
    quotes: [
      ...draft.quotes,
      {
        id,
        before: draft.text,
        quote: quote.quote,
        headings: [...quote.headings],
        reply: quote.reply,
      },
    ],
    text: '',
  };
}

/**
 * Remove a quote block. The free text on either side of it is the user's own
 * and stays: the text before it joins the text after it.
 */
export function removeDraftQuote(draft: ComposerDraft, id: string): ComposerDraft {
  const index = draft.quotes.findIndex((quote) => quote.id === id);
  if (index === -1) return draft;
  const removed = draft.quotes[index];
  const quotes = draft.quotes.filter((_, i) => i !== index);
  const next = quotes[index];
  if (next) {
    quotes[index] = { ...next, before: joinFreeText(removed.before, next.before) };
    return { quotes, text: draft.text };
  }
  return { quotes, text: joinFreeText(removed.before, draft.text) };
}

function joinFreeText(first: string, second: string): string {
  const a = first.trimEnd();
  const b = second.trimStart();
  if (!a) return second;
  if (!b) return first;
  return `${a}\n\n${b}`;
}

/** Selected text tidied for quoting: no edge whitespace, no runs of blank lines. */
export function normalizeQuoteText(text: string): string {
  return text
    .replace(/\r\n?/g, '\n')
    .replace(/[ \t ]+$/gm, '')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

// ---------------------------------------------------------------------------
// The sent message
// ---------------------------------------------------------------------------

/**
 * The line above each quote's blockquote, naming the note section it comes
 * from when it has one:
 *
 * ```text
 * <!-- note-quote: Architecture › Data flow -->
 * > The cache is invalidated on every write.
 * ```
 *
 * An HTML comment: the agent reads the path in it, a markdown renderer hides
 * it, and nobody types it by hand, so the transcript shows quote cards for
 * the blocks this composer wrote and nothing else. A `>` line or a `Re: …`
 * line someone typed or pasted stays the text it was.
 */
const QUOTE_MARKER = /^<!-- note-quote(?:: (.+?))? -->$/;

function quoteMarker(headings: string[]): string {
  if (headings.length === 0) return '<!-- note-quote -->';
  return `<!-- note-quote: ${headings.map(escapeHeading).join(HEADING_PATH_SEPARATOR)} -->`;
}

/** Characters a heading escapes in the marker: `›` would split the path, `>` could end the comment. */
const ESCAPED_IN_HEADING = /[›>]/g;

/**
 * Backslash-escape a heading for the marker, so a `›` or `-->` inside it
 * reads back as part of the heading. A backslash is doubled only where it
 * would otherwise read as an escape, so `C:\temp` stays as it is.
 */
function escapeHeading(heading: string): string {
  return heading.replace(/\\(?=[\\›>])/g, '\\\\').replace(ESCAPED_IN_HEADING, '\\$&');
}

/** A marker's heading path, split on unescaped `›`. */
function parseHeadingPath(path: string): string[] {
  const headings: string[] = [];
  let current = '';
  for (let i = 0; i < path.length; i++) {
    const char = path[i];
    const next = path[i + 1];
    if (char === '\\' && next !== undefined && '\\›>'.includes(next)) {
      current += next;
      i++;
    } else if (char === '›') {
      headings.push(current);
      current = '';
    } else {
      current += char;
    }
  }
  headings.push(current);
  return headings.map((heading) => heading.trim()).filter(Boolean);
}

/** A heading as it goes in the marker: on one line, no edge whitespace. */
function normalizeHeading(heading: string): string {
  return heading.replace(/\s+/g, ' ').trim();
}

function quoteMarkdown(quote: Pick<DraftQuote, 'quote' | 'headings'>): string {
  const lines = [quoteMarker(quote.headings.map(normalizeHeading).filter(Boolean))];
  for (const line of normalizeQuoteText(quote.quote).split('\n')) {
    lines.push(line ? `> ${line}` : '>');
  }
  return lines.join('\n');
}

/**
 * The message the agent receives: each quote as a markdown blockquote under
 * its marker line (see `QUOTE_MARKER`), followed by the reply, with the free
 * text in its original order around them. Blocks are separated by a blank
 * line.
 *
 * ```text
 * A few comments:
 *
 * <!-- note-quote: Architecture › Data flow -->
 * > The cache is invalidated on every write.
 *
 * Could we batch those?
 * ```
 *
 * A code fence left open in the free text before a quote, or in a reply, is
 * closed there: each is an input of its own, so the fence can't have been
 * meant to run on over the quote (and the agent would read the quote as code
 * if it did). A draft without quotes is sent exactly as typed, trimmed.
 */
export function serializeComposerDraft(draft: ComposerDraft): string {
  const parts: string[] = [];
  for (const quote of draft.quotes) {
    parts.push(closeOpenFence(quote.before.trim()));
    if (normalizeQuoteText(quote.quote)) parts.push(quoteMarkdown(quote));
    parts.push(closeOpenFence(quote.reply.trim()));
  }
  parts.push(draft.text.trim());
  return parts.filter(Boolean).join('\n\n');
}

// ---------------------------------------------------------------------------
// Fenced code
// ---------------------------------------------------------------------------

type CodeFence = { char: '`' | '~'; length: number };

/**
 * The fence a line opens, as CommonMark reads it: three or more backticks or
 * tildes, indented at most three spaces. A backtick fence's info string has
 * no backticks, so a line of inline code (```` ```x``` ````) opens nothing.
 */
function fenceOpenedBy(line: string): CodeFence | null {
  const match = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
  if (!match) return null;
  const char = match[1][0] as CodeFence['char'];
  if (char === '`' && match[2].includes('`')) return null;
  return { char, length: match[1].length };
}

/**
 * Whether a line closes `fence`: a run of its character at least as long as
 * the one that opened it, with nothing after but spaces. An info string
 * (```` ```js ````) makes the line content, not a close.
 */
function closesFence(line: string, fence: CodeFence): boolean {
  const match = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(line);
  return !!match && match[1][0] === fence.char && match[1].length >= fence.length;
}

/** The fence still open at the end of `text`, if any. */
function fenceOpenAtEnd(text: string): CodeFence | null {
  let fence: CodeFence | null = null;
  for (const line of text.split('\n')) {
    if (fence === null) fence = fenceOpenedBy(line);
    else if (closesFence(line, fence)) fence = null;
  }
  return fence;
}

function closeOpenFence(text: string): string {
  const fence = fenceOpenAtEnd(text);
  return fence ? `${text}\n${fence.char.repeat(fence.length)}` : text;
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/**
 * What `draftCache` persists for a draft. A draft without quotes is its plain
 * text, exactly as before quote blocks existed; one with quotes carries its
 * structure, with the serialized message as the text form.
 */
export function composerDraftContent(draft: ComposerDraft): string | DraftContent {
  if (draft.quotes.length === 0) return draft.text;
  const plain = cloneComposerDraft(draft);
  const structured: StoredComposerDraft = {
    version: STORED_DRAFT_VERSION,
    quotes: plain.quotes,
    text: plain.text,
  };
  return { value: serializeComposerDraft(plain), structured };
}

/** The draft a stored entry holds; a plain-text entry is all main-input text. */
export function composerDraftFromContent(content: DraftContent | null): ComposerDraft {
  if (!content) return emptyComposerDraft();
  return parseStoredComposerDraft(content.structured) ?? { quotes: [], text: content.value };
}

function parseStoredComposerDraft(value: unknown): ComposerDraft | null {
  if (!value || typeof value !== 'object') return null;
  const stored = value as Partial<StoredComposerDraft>;
  if (stored.version !== STORED_DRAFT_VERSION) return null;
  if (typeof stored.text !== 'string' || !Array.isArray(stored.quotes)) return null;
  const quotes: DraftQuote[] = [];
  for (const entry of stored.quotes as unknown[]) {
    const quote = parseStoredQuote(entry);
    if (!quote) return null;
    quotes.push(quote);
  }
  return { quotes, text: stored.text };
}

function parseStoredQuote(value: unknown): DraftQuote | null {
  if (!value || typeof value !== 'object') return null;
  const quote = value as Partial<DraftQuote>;
  if (
    typeof quote.id !== 'string' ||
    typeof quote.before !== 'string' ||
    typeof quote.quote !== 'string' ||
    typeof quote.reply !== 'string' ||
    !Array.isArray(quote.headings) ||
    !quote.headings.every((heading) => typeof heading === 'string')
  ) {
    return null;
  }
  return {
    id: quote.id,
    before: quote.before,
    quote: quote.quote,
    headings: [...quote.headings],
    reply: quote.reply,
  };
}

// ---------------------------------------------------------------------------
// Reading quotes back out of a sent message
// ---------------------------------------------------------------------------

export type QuotedMessagePart =
  { type: 'text'; text: string } | { type: 'quote'; quote: string; headings: string[] };

/** A line of a quote block: `> ` and its text, or a bare `>` for a blank line in it. */
const QUOTE_LINE = /^>(?: (.*))?$/;

function isBlankLine(line: string): boolean {
  return line.trim().length === 0;
}

/**
 * Split a user message into text and the quote blocks `serializeComposerDraft`
 * wrote, for the transcript. A block is its marker line (`QUOTE_MARKER`) and
 * the `>` lines straight after it, with a blank line or the message's edge on
 * either side; nothing inside a fenced code block counts. Everything else,
 * `>` lines included, is text and renders as typed.
 *
 * Returns a single text part when the message holds no quote, so the caller
 * can keep its plain rendering.
 */
export function parseQuotedMessage(text: string): QuotedMessagePart[] {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const parts: QuotedMessagePart[] = [];
  let buffer: string[] = [];
  let fence: CodeFence | null = null;

  const flushText = () => {
    const joined = buffer.join('\n').replace(/^\n+|\n+$/g, '');
    if (joined.trim()) parts.push({ type: 'text', text: joined });
    buffer = [];
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (fence !== null) {
      buffer.push(line);
      if (closesFence(line, fence)) fence = null;
      continue;
    }
    fence = fenceOpenedBy(line);
    const block = fence === null ? readQuoteBlock(lines, i) : null;
    if (!block) {
      buffer.push(line);
      continue;
    }
    flushText();
    parts.push({ type: 'quote', quote: block.quote, headings: block.headings });
    i = block.end - 1;
  }
  flushText();

  return parts;
}

/** The quote block whose marker is `lines[start]`, and the index of the line after it. */
function readQuoteBlock(
  lines: string[],
  start: number
): { quote: string; headings: string[]; end: number } | null {
  const marker = QUOTE_MARKER.exec(lines[start]);
  if (!marker || (start > 0 && !isBlankLine(lines[start - 1]))) return null;
  const quoteLines: string[] = [];
  let end = start + 1;
  for (; end < lines.length; end++) {
    const match = QUOTE_LINE.exec(lines[end]);
    if (!match) break;
    quoteLines.push(match[1] ?? '');
  }
  if (end < lines.length && !isBlankLine(lines[end])) return null;
  const quote = quoteLines.join('\n');
  if (!quote.trim()) return null;
  return { quote, headings: marker[1] ? parseHeadingPath(marker[1]) : [], end };
}

/** Whether a parsed message holds any quote (otherwise render it as plain text). */
export function hasQuotedParts(parts: QuotedMessagePart[]): boolean {
  return parts.some((part) => part.type === 'quote');
}
