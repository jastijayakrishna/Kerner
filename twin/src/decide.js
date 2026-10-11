// When a candidate has earned serving customers.
//
// The rule of three: after n comparisons with no unexplained difference, the true rate of unexplained differences is below 3/n with
// 95% confidence (from -ln 0.05 ≈ 3). So a candidate needs ceil(3/ε) clean comparisons for the bound ε.

/** The clean comparisons that bound the rate of unexplained differences below `epsilon`, with 95% confidence. */
export const needed = (epsilon) => Math.ceil(3 / epsilon - 1e-9);

/** A segment's counts over all its routes; the segment '*' adds up every segment. */
export function totals(snapshot, segment) {
  const out = { same: 0, stated: 0, different: 0, notCompared: 0, skipped: 0 };
  const names = segment === '*' ? Object.keys(snapshot.segments) : [segment];
  for (const name of names) {
    for (const counts of Object.values(snapshot.segments[name] ?? {})) {
      for (const key of Object.keys(out)) out[key] += counts[key];
    }
  }
  return out;
}

const percent = (x) => `${(x * 100).toFixed(1)}%`;

/**
 * Whether the candidate has earned serving customers, with every reason it has not.
 * policy: { epsilon, minHours, segments, maxUncovered }. Ready needs all of: no unexplained difference; in every listed segment at
 * least ceil(3/ε) clean comparisons (same or stated) and at most maxUncovered of its requests left uncompared (the bound says nothing
 * about requests it never saw); and minHours since the first comparison, so a full cycle of traffic was seen.
 */
export function decide(snapshot, policy, now) {
  const reasons = [];
  const need = needed(policy.epsilon);
  const bound = {};
  const all = totals(snapshot, '*');
  if (all.different) reasons.push(`${all.different} unexplained difference${all.different === 1 ? '' : 's'}`);
  for (const segment of policy.segments) {
    const t = totals(snapshot, segment);
    const clean = t.same + t.stated;
    bound[segment] = clean ? Math.min(1, 3 / clean) : 1;
    const label = segment === '*' ? 'all traffic' : `segment ${segment}`;
    const seen = clean + t.different + t.notCompared + t.skipped;
    const uncovered = t.notCompared + t.skipped;
    if (seen && uncovered / seen > policy.maxUncovered) reasons.push(`${label}: ${percent(uncovered / seen)} of requests could not be compared (at most ${percent(policy.maxUncovered)} may be)`);
    if (clean < need) reasons.push(`${label}: ${clean} of ${need} clean comparisons`);
  }
  const hours = snapshot.since ? Math.max(0, now - snapshot.since) / 3600000 : 0;
  if (hours < policy.minHours) reasons.push(`${hours.toFixed(1)} of ${policy.minHours} hours watched`);
  return { ready: reasons.length === 0, reasons, needed: need, bound };
}
