/**
 * How ProjectMatchReason shows a match reason's label: the terms highlighted
 * the way the label writes them, the text windowed so the first highlight
 * survives the line's ellipsis, and a tooltip that can't run to kilobytes.
 *
 * Plain module so the windowing is unit-testable without a Svelte runtime.
 */

import { highlightSegments, termPrNumber, type HighlightSegment } from '../../shared/textSearch';

/** A first highlight starting within this many characters shows the label
 *  from its start: it fits even in the sidebar's narrow meta line. */
export const MATCH_LEAD_CHARS = 30;

/** How much text to keep before a later first highlight: the whole words
 *  that start within this many characters of it. */
export const MATCH_CONTEXT_CHARS = 16;

/** The tooltip's longest label. A note stub is titled with the prompt that
 *  started its session, which can be several kilobytes. */
export const TOOLTIP_MAX_CHARS = 300;

/**
 * The query's terms (queryTerms) as the label writes them. `pr123` and
 * `pr#123` matched a PR by its number, and the label reads `PR #123`, so
 * they highlight as `123` and `#123`.
 */
export function labelTerms(terms: readonly string[]): string[] {
  return terms.map((term) => (termPrNumber(term) === null ? term : term.replace(/^pr/, '')));
}

const isSpace = (ch: string) => /\s/.test(ch);

/**
 * Where to start showing `label` so the highlight at `start` keeps a little
 * context: the start of its word, then of each earlier word that begins
 * within MATCH_CONTEXT_CHARS of it. A match deep inside one long word starts
 * the window at the match itself.
 */
function windowStart(label: string, start: number): number {
  let cut = start;
  while (cut > 0 && !isSpace(label[cut - 1])) cut--;
  if (start - cut > MATCH_CONTEXT_CHARS) return start;
  for (;;) {
    let word = cut;
    while (word > 0 && isSpace(label[word - 1])) word--;
    while (word > 0 && !isSpace(label[word - 1])) word--;
    if (word === cut || start - word > MATCH_CONTEXT_CHARS) return cut;
    cut = word;
  }
}

/**
 * `label` split into highlighted and plain runs (highlightSegments). When
 * the first highlight starts past MATCH_LEAD_CHARS, the runs start a few
 * words before it instead, behind a leading `…`: the line ellipsises its
 * end, and a long title would otherwise hide the very term that explains
 * the match.
 */
export function matchLabelSegments(label: string, terms: readonly string[]): HighlightSegment[] {
  const segments = highlightSegments(label, terms);
  const first = segments.findIndex((s) => s.match);
  if (first === -1) return segments;
  let start = 0;
  for (let i = 0; i < first; i++) start += segments[i].text.length;
  if (start <= MATCH_LEAD_CHARS) return segments;

  const cut = windowStart(label, start);
  const windowed: HighlightSegment[] = [];
  let offset = 0;
  for (const segment of segments) {
    const end = offset + segment.text.length;
    if (end > cut) {
      windowed.push(
        offset >= cut ? segment : { text: segment.text.slice(cut - offset), match: segment.match }
      );
    }
    offset = end;
  }
  if (windowed[0].match) windowed.unshift({ text: '…', match: false });
  else windowed[0] = { text: `…${windowed[0].text}`, match: false };
  return windowed;
}

/** `text` cut to `max` characters, the last one an `…`, when it's longer. */
export function clampText(text: string, max = TOOLTIP_MAX_CHARS): string {
  if (text.length <= max) return text;
  let end = max - 1;
  // Don't split a surrogate pair.
  const code = text.charCodeAt(end - 1);
  if (code >= 0xd800 && code <= 0xdbff) end--;
  return `${text.slice(0, end).trimEnd()}…`;
}
