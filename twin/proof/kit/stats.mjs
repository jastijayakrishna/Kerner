export function percentile(values, p) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(1, Math.ceil((p / 100) * sorted.length)) - 1];
}

const round = (x) => (x === null ? null : Math.round(x * 100) / 100);

export function summarize(ms) {
  return { count: ms.length, p50: round(percentile(ms, 50)), p90: round(percentile(ms, 90)), p99: round(percentile(ms, 99)), max: round(percentile(ms, 100)) };
}

export function comparedShare(stats) {
  const compared = stats.same + stats.stated + stats.different;
  const judged = compared + stats.notCompared + stats.skipped + (stats.dropped ?? 0);
  return { compared, judged, share: judged ? Math.round((compared / judged) * 10000) / 10000 : null };
}
