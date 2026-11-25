// How out of date the subtitles are, sampled on a timer while a window is
// measured.
//
// The latency figures time each piece of output that reaches the page, from
// its own write. A page that shows one piece in three scores as well on
// them as a page that shows every piece, while its rows sit on older text
// for longer. This asks the other question: at a given moment, for how long
// has the store held something newer than a row shows?
//
// Each sample reads what every row shows, with the page's clock, and then
// asks the app when the first newer piece of each session was stored. The
// app answers from memory (`/perf/outdated`), so sampling takes no store
// lock and makes no command; page, app and agents share one clock. A piece
// stored after the page was read does not count against the row.

import { outdatedFor } from './stamps.mjs';
import { distribution } from './stats.mjs';
import * as ui from './ui.mjs';
import { sleep } from './util.mjs';

export const SAMPLE_EVERY_MS = 500;

/**
 * Start sampling `sessions` (`[{ tag, branchId }]`). `stop()` ends it and
 * returns the figures, or `null` when the app's hooks cannot answer or no
 * sample could be taken.
 */
export function startStaleness({ api, frontend, sessions, every = SAMPLE_EVERY_MS }) {
  const ms = [];
  const pieces = [];
  const mounted = new Set();
  let samples = 0;
  let nothingShown = 0;
  let floors = 0;
  let unsupported = false;
  let stopped = false;
  let failure = null;

  const loop = (async () => {
    while (!stopped) {
      const began = Date.now();
      try {
        const { at, rows } = await frontend.evaluate(ui.rows(sessions));
        const shown = rows.filter((row) => row.row);
        const answers = await api.outdated(
          Object.fromEntries(shown.map((row) => [row.tag, row.seq ?? 0])),
          at
        );
        if (answers === null) {
          unsupported = true;
          return;
        }
        if (stopped) break;
        samples += 1;
        for (const row of shown) {
          const answer = answers[row.tag];
          // Nothing of the session is stored yet: nothing to be behind.
          if (!answer) continue;
          mounted.add(row.tag);
          const gap = outdatedFor(answer, at);
          if (row.seq === null && gap.behind) nothingShown += 1;
          if (gap.floor) floors += 1;
          ms.push(gap.ms);
          // Hooks older than the count answer without one.
          if (gap.pieces !== null) pieces.push(gap.pieces);
        }
      } catch (error) {
        // The window's own collection reports a lost page or app.
        failure = error.message;
        return;
      }
      await sleep(Math.max(0, every - (Date.now() - began)));
    }
  })();

  return {
    async stop() {
      stopped = true;
      await loop;
      if (unsupported || samples === 0) return null;
      return {
        samples,
        rows: mounted.size,
        outdatedMs: distribution(ms),
        piecesBehind: distribution(pieces),
        upToDate: ms.filter((value) => value === 0).length,
        over1s: ms.filter((value) => value > 1000).length,
        // Rows that showed no stamp while the store held one for them.
        nothingShown,
        // Samples further behind than the app keeps track of: their figure is a least value.
        floors,
        stoppedEarly: failure,
      };
    },
  };
}
