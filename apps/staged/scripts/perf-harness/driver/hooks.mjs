// The version of the app's harness hooks this driver was written against,
// and what a result lacks when the app's are older. A tree measured through
// `--app-dir` can carry older hooks than the harness that drives it.
//
// See `HOOKS_VERSION` in the app's `perf_harness/metrics.rs`.

export const HOOKS_WANTED = 3;

const MISSING = [
  [
    2,
    'there is no subtitle staleness figure, tool output is counted with text in agentWriteToStoredMs, and the backend percentiles are coarse (half an octave)',
  ],
  [
    3,
    'only `/perf/*` of the app’s port asks for the run’s token (its web API answered any local process during this run), a tool call whose row holds its stamp twice is counted twice in agentWriteToStoredMs.tool and reaches half as far back in the staleness record, subtitleStaleness has no piecesBehind, and backend.streamTotals counts a frame as sent whether or not a client took it (no `dropped`)',
  ],
];

/** What to tell the reader of a result from hooks of version `hooks`, or `null` when nothing is missing. */
export function olderHooksWarning(hooks) {
  const missing = MISSING.filter(([version]) => hooks < version).map(([, what]) => what);
  if (missing.length === 0) return null;
  return `the app’s harness hooks are version ${hooks} and this harness expects ${HOOKS_WANTED}: ${missing.join('; ')}`;
}
