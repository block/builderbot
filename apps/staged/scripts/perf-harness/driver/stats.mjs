// Turning samples into the figures a result file holds.

const round = (value, places = 1) =>
  value === null || value === undefined ? null : Number(value.toFixed(places));

/** `{ count, p50, p95, p99, max, mean }` of a list of numbers, or `null` for an empty one. */
export function distribution(values, places = 1) {
  if (!values || values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const at = (p) => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
  return {
    count: sorted.length,
    p50: round(at(0.5), places),
    p95: round(at(0.95), places),
    p99: round(at(0.99), places),
    max: round(sorted[sorted.length - 1], places),
    mean: round(sorted.reduce((a, b) => a + b, 0) / sorted.length, places),
  };
}

/**
 * The same figures in milliseconds from one of the app's histograms:
 * `{ count, sumUs, maxUs, buckets: [[upperUs, n], ...] }`. A percentile is
 * the upper bound of the bucket it falls in, so it is never under the true
 * value and over it by less than 6.25 % (sixteen buckets to the octave; the
 * first version of the app's hooks had two, so within 41 %). `max` and
 * `mean` are exact.
 */
export function histogramMs(histogram) {
  if (!histogram || histogram.count === 0) return null;
  const at = (p) => {
    let seen = 0;
    for (const [upperUs, n] of histogram.buckets) {
      seen += n;
      if (seen > p * histogram.count) return Math.min(upperUs, histogram.maxUs) / 1000;
    }
    return histogram.maxUs / 1000;
  };
  return {
    count: histogram.count,
    p50: round(at(0.5), 3),
    p95: round(at(0.95), 3),
    p99: round(at(0.99), 3),
    max: round(histogram.maxUs / 1000, 3),
    mean: round(histogram.sumUs / histogram.count / 1000, 3),
    totalMs: round(histogram.sumUs / 1000, 1),
  };
}

/**
 * CPU and memory of a set of processes over a window, from
 * `{ t, cpuSeconds, rssKb }` samples taken once a second, `cpuSeconds` a
 * running total (see `cpuMeter` in `sampler.mjs`). The CPU is what was used
 * between the first and the last sample, and the percentage is of that
 * span (`spanSeconds`), not of the window: the samples nearest the window's
 * ends can each be half a second inside or outside it. `samples` says how
 * many there were: far fewer than the window's seconds means the sampler
 * itself was starved and the figures rest on little.
 */
export function processUsage(samples) {
  if (samples.length < 2) return null;
  const [first, last] = [samples[0], samples[samples.length - 1]];
  const cpu = last.cpuSeconds - first.cpuSeconds;
  const span = (last.t - first.t) / 1000;
  const rss = samples.map((s) => s.rssKb / 1024);
  return {
    cpuSeconds: round(cpu, 2),
    cpuPercent: span > 0 ? round((cpu / span) * 100, 1) : null,
    spanSeconds: round(span, 2),
    rssMaxMb: round(Math.max(...rss), 1),
    rssMeanMb: round(rss.reduce((a, b) => a + b, 0) / rss.length, 1),
    processesMax: Math.max(...samples.map((s) => s.processes)),
    samples: samples.length,
  };
}

export { round };
