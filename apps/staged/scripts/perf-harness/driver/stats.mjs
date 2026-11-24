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
 * the upper bound of the bucket it falls in, so it is exact to half an
 * octave and never under the true value; `max` and `mean` are exact.
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

/** CPU and memory of a set of processes over a window, from `{ t, cpuSeconds, rssKb }` samples. */
export function processUsage(samples, seconds) {
  if (samples.length < 2) return null;
  const cpu = samples[samples.length - 1].cpuSeconds - samples[0].cpuSeconds;
  const rss = samples.map((s) => s.rssKb / 1024);
  return {
    cpuSeconds: round(cpu, 2),
    cpuPercent: round((cpu / seconds) * 100, 1),
    rssMaxMb: round(Math.max(...rss), 1),
    rssMeanMb: round(rss.reduce((a, b) => a + b, 0) / rss.length, 1),
    processesMax: Math.max(...samples.map((s) => s.processes)),
  };
}

export { round };
