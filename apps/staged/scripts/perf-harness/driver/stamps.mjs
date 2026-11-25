// The fake agent's stamps, read outside the page: in a stored transcript,
// and set against the app's record of when each was stored. `probe.js` and
// `ui.mjs` read the same format inside the page.
//
//   pfm-<kind><tag>-<seq>-<epoch ms>    kind x text, t tool call, o tool output
//
// A session's sequence runs through all three kinds. Its row shows the
// newest `x` or `t` as its subtitle and never an `o`.

const STAMP = /pfm-([xto])([a-z0-9]+)-(\d+)-(\d{13})/g;

/** Every stamp in a text, in order: `[{ kind, tag, seq, sent }]`. */
export function stampsIn(text) {
  const out = [];
  if (!text || !text.includes('pfm-')) return out;
  for (const m of text.matchAll(STAMP))
    out.push({ kind: m[1], tag: m[2], seq: Number(m[3]), sent: Number(m[4]) });
  return out;
}

/**
 * How out of date a row is: `{ behind, ms, pieces, floor }`.
 *
 * `answer` is the app's (`/perf/outdated`) for the stamp the row showed at
 * `at` (the page's clock, epoch ms): the first stamp stored after that one,
 * when it was stored, and how many the store held at `at`. `behind` says
 * the store held something newer at `at`; `ms` for how long it had, and 0
 * when it had not. `pieces` is how many pieces the row could have shown and
 * did not: the app's count of showable stamps (text and tool calls) newer
 * than the one shown. It is not a difference of sequence numbers, which
 * would count a tool call's output steps, and those are never shown. `null`
 * from hooks that do not count. `floor` says the row was further behind
 * than the app keeps track of, so `ms` and `pieces` are least values.
 */
export function outdatedFor(answer, at) {
  if (answer.newerSeq === null || answer.newerStoredMs > at)
    return { behind: false, ms: 0, pieces: 0, floor: false };
  return {
    behind: true,
    ms: at - answer.newerStoredMs,
    pieces: answer.newerCount ?? null,
    floor: answer.floor,
  };
}

/**
 * What a transcript holds for one session, from its rows as the app's
 * `get_session_messages` returns them: the text stamps of its assistant
 * rows and the stamps of its tool calls, each `seq → sent`, and the newest
 * of both, which is what the session's row should come to show.
 */
export function storedStamps(rows, tag) {
  const text = new Map();
  const tool = new Map();
  let newest = null;
  for (const row of rows) {
    const into = row.role === 'assistant' ? text : row.role === 'tool_call' ? tool : null;
    if (!into) continue;
    const kind = into === text ? 'x' : 't';
    for (const stamp of stampsIn(row.content)) {
      if (stamp.kind !== kind || stamp.tag !== tag) continue;
      into.set(stamp.seq, stamp.sent);
      if (!newest || stamp.seq > newest.seq) newest = { kind, seq: stamp.seq, sent: stamp.sent };
    }
  }
  return { text, tool, newest };
}
