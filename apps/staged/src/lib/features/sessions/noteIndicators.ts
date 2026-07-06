export type NoteIndicatorSplit = {
  preamble: string;
  hasNote: boolean;
};

type SplitOptions = {
  streaming?: boolean;
};

const SUGGESTED_NEXT_STEPS_MARKER = '```suggested-next-steps';
const HR_MARKERS = ['---', '***', '___'] as const;

/**
 * Frontend companion to `extract_note_content` in `src-tauri/src/session_runner.rs`.
 * Keep this detection aligned with the backend parser so chat rendering and
 * note persistence agree on where note output begins.
 */
export function splitAtNoteIndicator(text: string, options: SplitOptions = {}): NoteIndicatorSplit {
  const candidates = [
    findSuggestedNextStepsIndicator(text, options.streaming ?? false),
    findStandaloneHrIndicator(text, options.streaming ?? false),
    findInlineHrIndicator(text),
  ].filter((index): index is number => index !== null);

  if (candidates.length === 0) {
    return { preamble: text, hasNote: false };
  }

  const indicatorIndex = Math.min(...candidates);
  return {
    preamble: stripSuggestedNextStepsBlocks(text.slice(0, indicatorIndex)),
    hasNote: true,
  };
}

function findSuggestedNextStepsIndicator(text: string, streaming: boolean): number | null {
  let searchFrom = 0;
  while (searchFrom < text.length) {
    const index = text.indexOf(SUGGESTED_NEXT_STEPS_MARKER, searchFrom);
    if (index === -1) return null;

    const afterMarker = text.slice(index + SUGGESTED_NEXT_STEPS_MARKER.length);
    const newlineIndex = afterMarker.indexOf('\n');
    if (newlineIndex === -1) {
      if (streaming && afterMarker.trim().length === 0) return index;
    } else if (afterMarker.slice(0, newlineIndex).trim().length === 0) {
      if (streaming) return index;
      const contentStart = index + SUGGESTED_NEXT_STEPS_MARKER.length + newlineIndex + 1;
      if (findClosingFence(text.slice(contentStart)) !== null) return index;
    }

    searchFrom = index + SUGGESTED_NEXT_STEPS_MARKER.length;
  }
  return null;
}

function stripSuggestedNextStepsBlocks(text: string): string {
  let output = '';
  let lastCopied = 0;
  let searchFrom = 0;
  let removedAny = false;

  while (searchFrom < text.length) {
    const start = findSuggestedNextStepsIndicator(text.slice(searchFrom), false);
    if (start === null) break;

    const startIndex = searchFrom + start;
    const blockStart = startIndex + SUGGESTED_NEXT_STEPS_MARKER.length;
    const newlineIndex = text.slice(blockStart).indexOf('\n');
    if (newlineIndex === -1) break;

    const contentStart = blockStart + newlineIndex + 1;
    const closingRelative = findClosingFence(text.slice(contentStart));
    if (closingRelative === null) break;

    const closingStart = contentStart + closingRelative;
    const afterClosingNewline = text.slice(closingStart).indexOf('\n');
    const afterClosing =
      afterClosingNewline === -1 ? text.length : closingStart + afterClosingNewline + 1;

    output += text.slice(lastCopied, startIndex);
    lastCopied = afterClosing;
    searchFrom = afterClosing;
    removedAny = true;
  }

  if (!removedAny) return text;
  return output + text.slice(lastCopied);
}

function findStandaloneHrIndicator(text: string, streaming: boolean): number | null {
  let insideFence = false;
  for (const line of lineRanges(text)) {
    const trimmed = line.text.trim();
    if (trimmed.startsWith('```') || trimmed.startsWith('~~~')) {
      insideFence = !insideFence;
      continue;
    }
    if (insideFence || !isHrLine(trimmed)) continue;

    const remaining = text.slice(line.endWithNewline).trim();
    if (remaining.length > 0 || streaming) {
      return line.start;
    }
  }
  return null;
}

function findInlineHrIndicator(text: string): number | null {
  const fenceRanges = computeFenceRanges(text);
  let best: number | null = null;

  for (const marker of HR_MARKERS) {
    const markerChar = marker[0];
    let searchFrom = 0;
    while (searchFrom < text.length) {
      const index = text.indexOf(marker, searchFrom);
      if (index === -1) break;

      const markerEnd = index + marker.length;
      const inFence = fenceRanges.some(([start, end]) => index >= start && index < end);
      const longerRun = text[index - 1] === markerChar || text[markerEnd] === markerChar;
      const remaining = text.slice(markerEnd).trimStart();

      if (!inFence && !longerRun && remaining.startsWith('# ')) {
        best = best === null ? index : Math.min(best, index);
      }

      searchFrom = markerEnd;
    }
  }

  return best;
}

function computeFenceRanges(text: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  let fenceStart: number | null = null;

  for (const line of lineRanges(text)) {
    const trimmed = line.text.trim();
    if (!trimmed.startsWith('```') && !trimmed.startsWith('~~~')) continue;

    if (fenceStart !== null) {
      ranges.push([fenceStart, line.start]);
      fenceStart = null;
    } else {
      fenceStart = line.endWithNewline;
    }
  }

  return ranges;
}

function findClosingFence(text: string): number | null {
  let searchFrom = 0;
  while (searchFrom < text.length) {
    const index = text.indexOf('```', searchFrom);
    if (index === -1) return null;

    const atLineStart = index === 0 || text[index - 1] === '\n';
    const after = text.slice(index + 3);
    const nextNewline = after.indexOf('\n');
    const closes =
      atLineStart &&
      (after.length === 0 ||
        (nextNewline === -1
          ? after.trim().length === 0
          : after.slice(0, nextNewline).trim().length === 0));

    if (closes) return index;
    searchFrom = index + 3;
  }
  return null;
}

function isHrLine(trimmed: string): boolean {
  return HR_MARKERS.includes(trimmed as (typeof HR_MARKERS)[number]);
}

function lineRanges(text: string): Array<{ start: number; endWithNewline: number; text: string }> {
  const lines: Array<{ start: number; endWithNewline: number; text: string }> = [];
  let start = 0;

  while (start <= text.length) {
    const newlineIndex = text.indexOf('\n', start);
    if (newlineIndex === -1) {
      lines.push({ start, endWithNewline: text.length, text: text.slice(start) });
      break;
    }

    lines.push({
      start,
      endWithNewline: newlineIndex + 1,
      text: text.slice(start, newlineIndex),
    });
    start = newlineIndex + 1;
  }

  return lines;
}
