// Telling a run that measures the app from one that has only overloaded the
// machine. Past the app's limit the fake agents (one process each) fall
// behind the rate they were asked for and the once-a-second process sampler
// stops getting its turns; the figures of such a window describe the
// machine, and the run takes it down for everything else. So the driver
// checks both while a window is measured and stops the run when either
// falls under `--min-rate-share`.

/**
 * The chunks a session writes per second when nothing holds it back. A tool
 * call counts as a chunk and takes `toolMs` in place of its interval.
 */
export function expectedChunksPerSecond({ chunksPerSecond, toolEvery, toolMs }) {
  if (!(toolEvery > 0)) return chunksPerSecond;
  return toolEvery / ((toolEvery - 1) / chunksPerSecond + toolMs / 1000);
}

/**
 * Why a window is overloaded, or `null`. `chunks` were written and
 * `samples` taken in `seconds`; `expectedPerSecond` is the sum over the
 * sessions. A `minShare` of 0 never stops a run.
 */
export function overloadReason({ chunks, expectedPerSecond, samples, seconds, minShare }) {
  if (!(minShare > 0) || !(seconds > 0)) return null;
  const share = chunks / (expectedPerSecond * seconds);
  if (expectedPerSecond > 0 && share < minShare) {
    return `the agents wrote ${Math.round(chunks / seconds)} chunks a second, ${(share * 100).toFixed(0)}% of the ${Math.round(expectedPerSecond)} asked for`;
  }
  // The sampler is due once a second.
  if (samples / seconds < minShare) {
    return `the process sampler took ${samples} samples in ${Math.round(seconds)}s`;
  }
  return null;
}
