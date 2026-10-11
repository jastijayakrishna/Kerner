// Numbers for the proof: request times summed up exactly (a run has at most a few thousand requests, so they are simply sorted), and
// the share of sampled requests the twin compared.

/** The p-th percentile (0 to 100) of some numbers, by the nearest-rank method; null when there are none. */
export function percentile(values, p) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(1, Math.ceil((p / 100) * sorted.length)) - 1];
}

const round = (x) => (x === null ? null : Math.round(x * 100) / 100);

/** How many requests, and their p50, p90, p99 and slowest time, in milliseconds to 0.01. */
export function summarize(ms) {
  return { count: ms.length, p50: round(percentile(ms, 50)), p90: round(percentile(ms, 90)), p99: round(percentile(ms, 99)), max: round(percentile(ms, 100)) };
}

/** Of the sampled requests the twin judged, how many it compared (same, stated or different), and that share to 4 places. */
export function comparedShare(stats) {
  const compared = stats.same + stats.stated + stats.different;
  const judged = compared + stats.notCompared + stats.skipped;
  return { compared, judged, share: judged ? Math.round((compared / judged) * 10000) / 10000 : null };
}
