// When a candidate has earned serving customers.
//
// The rule of three: after n comparisons with no unexplained difference, the true rate of unexplained differences is below 3/n with
// 95% confidence (from -ln 0.05 ≈ 3). So a candidate needs ceil(3/ε) clean comparisons for the bound ε. With a manifest (Phase 3) every
// changed line, old and new, must also have run, every part of it, in minLineRuns clean comparisons.

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
 * What the evidence says of one changed line.
 * - 'accepted': a person accepted it.
 * - 'at load': it ran while the version loaded.
 * - 'proven': every code character ran in at least minLineRuns clean comparisons.
 * - 'run': every character ran in a clean comparison, but not that often yet.
 * - 'partly run': some characters never ran in a clean comparison.
 * - 'never run'.
 */
export function lineStatus(snapshot, key, policy, loaded) {
  if (policy.accepted?.(key)) return 'accepted';
  if (loaded.has(key)) return 'at load';
  const entry = snapshot.lines?.[key];
  if (!entry || !entry.touched) return 'never run';
  const digits = [...entry.cover].map(Number);
  if (digits.length && digits.every((d) => d >= policy.minLineRuns)) return 'proven';
  if (digits.length && digits.every((d) => d >= 1)) return 'run';
  return 'partly run';
}

/** Every changed line (at most 2,000) with its status and counts. `lines` is the manifest's Map of line keys. */
export function pathsOf(snapshot, lines, policy) {
  const loaded = new Set(snapshot.loadLines ?? []);
  const out = [];
  for (const [key, info] of lines) {
    if (out.length >= 2000) break;
    const entry = snapshot.lines?.[key];
    out.push({
      key, side: info.side, path: info.path, line: info.line, text: info.text, pull: info.pull,
      touched: entry?.touched ?? 0, runs: entry?.runs ?? 0, clean: entry?.clean ?? 0, status: lineStatus(snapshot, key, policy, loaded),
    });
  }
  return out;
}

/**
 * Whether the candidate has earned serving customers, with every reason it has not.
 *
 * The policy is { epsilon, minHours, segments, maxUncovered, minLineRuns?, accepted?, lineKeys?, known? }. Ready needs all of:
 * - no unexplained difference;
 * - in every listed segment, at least ceil(3/ε) clean comparisons (same or stated);
 * - in every listed segment, at most maxUncovered of its requests left uncompared, since the bound says nothing about requests it
 *   never saw;
 * - minHours since the first comparison;
 * - with lineKeys, every changed line 'proven', 'at load' or 'accepted'. The others are listed in `waiting`.
 * - with known cases (Phase 5), every case checked: none still showing a difference, none waiting for a person, every file read.
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
  const known = policy.known;
  if (known) {
    const names = (list) => `${list.slice(0, 5).join(', ')}${list.length > 5 ? ', ...' : ''}`;
    const one = (list) => list.length === 1;
    if (known.pending) reasons.push(`${known.pending} known case${known.pending === 1 ? '' : 's'} not yet checked`);
    if (known.failing.length) reasons.push(`${known.failing.length} known case${one(known.failing) ? ' still shows' : 's still show'} a difference: ${names(known.failing)}`);
    if (known.stubs.length) reasons.push(`${known.stubs.length} known difference${one(known.stubs) ? ' needs a person, who removes its file' : 's need a person, who removes their files'} once done: ${names(known.stubs)}`);
    if (known.unreadable.length) reasons.push(`${known.unreadable.length} known case file${one(known.unreadable) ? '' : 's'} could not be read: ${names(known.unreadable)}`);
  }
  const hours = snapshot.since ? Math.max(0, now - snapshot.since) / 3600000 : 0;
  if (hours < policy.minHours) reasons.push(`${hours.toFixed(1)} of ${policy.minHours} hours watched`);
  const result = { ready: false, reasons, needed: need, bound };
  if (policy.lineKeys?.length) {
    const loaded = new Set(snapshot.loadLines ?? []);
    const waiting = policy.lineKeys.filter((key) => !['accepted', 'at load', 'proven'].includes(lineStatus(snapshot, key, policy, loaded)));
    result.waiting = waiting;
    if (waiting.length) {
      const list = `${waiting.slice(0, 5).join(', ')}${waiting.length > 5 ? ', ...' : ''}`;
      reasons.push(`${waiting.length} changed line${waiting.length === 1 ? ' has' : 's have'} not yet run in full in ${policy.minLineRuns} clean comparisons: ${list}`);
    }
  }
  result.ready = reasons.length === 0;
  return result;
}
