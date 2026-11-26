// Checks that what the page shows is what the store holds. The rest of the
// harness measures delay; a page that showed the wrong text quickly, or
// stopped following a session, would measure well.
//
// Both checks run after a scenario's measured window has closed, so the
// store reads they make (through the app's own `get_session_messages`) are
// in no figure.
//
// - Subtitles, while the sessions are still running: read the newest text or
//   tool-call stamp stored for each session whose row is mounted, then
//   watch the page until every row shows that stamp or a newer one. Every
//   stamp a row showed on the way must be one the store holds, with the
//   time the agent gave it.
// - The chat, once its session has stopped and the store no longer moves:
//   the text stamps in the open chat against the text stamps of the stored
//   transcript. The chat holds a window of the transcript, so from the
//   oldest stamp it shows to the newest one stored there must be no stamp
//   missing and none the store does not hold.

import { storedStamps } from './stamps.mjs';
import * as ui from './ui.mjs';
import { sleep } from './util.mjs';

/** The newest rows are enough to find what a subtitle shows; a row holds a whole message. */
const SUBTITLE_ROWS = 60;
const SUBTITLE_TIMEOUT_MS = 8000;
const CHAT_TIMEOUT_MS = 15_000;
/** At most this many of a long list go into a result. */
const LISTED = 20;

const stored = async (api, session, limit) =>
  storedStamps(
    await api.invoke('get_session_messages', { sessionId: session.id, ...(limit && { limit }) }),
    session.tag
  );

/**
 * One round of the subtitle check over `sessions` (`[{ id, tag, branchId }]`).
 * Returns `{ rows, caughtUpMs, late, unknown }`: how many rows were checked,
 * how long each took to show what the store held when the round began, the
 * rows that never did, and the stamps shown that the store does not hold.
 */
export async function subtitleRound({ api, frontend, sessions, timeout = SUBTITLE_TIMEOUT_MS }) {
  const mounted = (await frontend.evaluate(ui.rows(sessions))).rows.filter((row) => row.row);
  const byTag = new Map(sessions.map((session) => [session.tag, session]));
  const targets = new Map();
  for (const { tag } of mounted) {
    const { newest } = await stored(api, byTag.get(tag), SUBTITLE_ROWS);
    if (newest) targets.set(tag, newest);
  }
  const began = Date.now();
  const caughtUp = new Map();
  const shown = new Map(); // tag -> seq -> { kind, sent }
  for (;;) {
    for (const row of (await frontend.evaluate(ui.rows(sessions))).rows) {
      if (!targets.has(row.tag) || row.seq === null) continue;
      const seen = shown.get(row.tag) ?? shown.set(row.tag, new Map()).get(row.tag);
      seen.set(row.seq, { kind: row.kind, sent: row.sent });
      if (!caughtUp.has(row.tag) && row.seq >= targets.get(row.tag).seq)
        caughtUp.set(row.tag, Date.now() - began);
    }
    if (caughtUp.size === targets.size || Date.now() - began > timeout) break;
    await sleep(100);
  }
  // Read again, now that the store holds whatever the rows went on to show.
  const unknown = [];
  for (const [tag, seen] of shown) {
    const now = await stored(api, byTag.get(tag), SUBTITLE_ROWS);
    for (const [seq, { kind, sent }] of seen) {
      if ((kind === 'x' ? now.text : now.tool).get(seq) !== sent)
        unknown.push(`pfm-${kind}${tag}-${seq}-${sent}`);
    }
  }
  const late = [...targets.keys()]
    .filter((tag) => !caughtUp.has(tag))
    .map((tag) => {
      const seen = shown.get(tag);
      const target = targets.get(tag);
      return `${tag}: shows ${seen ? `seq ${Math.max(...seen.keys())}` : 'no stamp'}, the store held ${target.kind}${target.seq} when the round began`;
    });
  return { rows: targets.size, caughtUpMs: [...caughtUp.values()], late, unknown };
}

/** `rounds` subtitle rounds a second apart, as one check. */
export async function verifySubtitles({ api, frontend, sessions, rounds = 3 }) {
  const out = { rounds: 0, rows: 0, caughtUpMsMax: null, late: [], unknown: [] };
  for (let round = 0; round < rounds; round++) {
    if (round > 0) await sleep(1000);
    const result = await subtitleRound({ api, frontend, sessions });
    out.rounds += 1;
    out.rows = Math.max(out.rows, result.rows);
    out.caughtUpMsMax = Math.max(out.caughtUpMsMax ?? 0, ...result.caughtUpMs);
    out.late.push(...result.late.map((line) => `round ${round + 1}: ${line}`));
    out.unknown.push(...result.unknown);
  }
  return finish(out, [out.late, out.unknown], out.rows > 0);
}

/**
 * The chat against the transcript, as counts and lists:
 * `shown` and `stored` are `seq → sent` of the text stamps on each side.
 */
export function compareChat({ shown, stored }) {
  const unknown = [...shown].filter(([seq, sent]) => stored.get(seq) !== sent).map(([seq]) => seq);
  const oldest = shown.size ? Math.min(...shown.keys()) : Infinity;
  const missing = [...stored.keys()].filter((seq) => seq >= oldest && !shown.has(seq));
  return {
    shown: shown.size,
    stored: stored.size,
    newestShown: shown.size ? Math.max(...shown.keys()) : null,
    newestStored: stored.size ? Math.max(...stored.keys()) : null,
    unknown,
    missing,
  };
}

/** The open chat of a stopped session against its stored transcript. */
export async function verifyChat({ api, frontend, session, timeout = CHAT_TIMEOUT_MS }) {
  const { text } = await stored(api, session);
  const began = Date.now();
  let compared;
  for (;;) {
    const stamps = await frontend.evaluate(ui.chatStamps(session.tag));
    compared = compareChat({ shown: new Map(stamps ?? []), stored: text });
    const settled = compared.unknown.length === 0 && compared.missing.length === 0;
    if ((settled && compared.shown > 0) || Date.now() - began > timeout) break;
    await sleep(250);
  }
  return finish(
    { ...compared, settledMs: Date.now() - began },
    [compared.unknown, compared.missing],
    compared.shown > 0
  );
}

/** `ok` is false when any list of problems has an entry or there was nothing to check; long lists are cut. */
function finish(check, problems, checkedSomething) {
  const ok = checkedSomething && problems.every((list) => list.length === 0);
  for (const [key, value] of Object.entries(check)) {
    if (Array.isArray(value) && value.length > LISTED) {
      check[`${key}Count`] = value.length;
      check[key] = value.slice(0, LISTED);
    }
  }
  return { ok, ...check };
}
