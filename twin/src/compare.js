import { diffValues, readBody } from './serialize.js';
import { shapeOf } from './keys.js';

/** Headers that change on every answer and say nothing about behaviour. */
export const VOLATILE_HEADERS = new Set(['date', 'etag', 'last-modified', 'age', 'expires', 'content-length', 'connection', 'keep-alive', 'transfer-encoding', 'x-request-id', 'x-response-time', 'server-timing', 'set-cookie']);

/**
 * Kinds only a rule that names them can state: a path rule never makes a crash, a hang, an opened door, another write or a reach for
 * something a ghost may not use "meant".
 */
const SERIOUS = new Set(['crash', 'hang', 'door-opened', 'effects', 'refused']);
const KEPT = 20;

const refused = (status) => status === 401 || status === 403 || status === 404 || status === 405 || (status >= 300 && status < 400);
const succeeded = (status) => status >= 200 && status < 300;
const writes = (list) => [...(list ?? [])].sort().join('\n');
// The real run records calls (fetch, http, Postgres, Redis), not file writes, and its payloads carry the real clock: what it wrote is
// compared with the serving ghost by shape, and file writes are compared between the two ghosts only.
const recordedShapes = (list) => [...(list ?? [])].filter((effect) => !effect.startsWith('FS ')).map(shapeOf).sort().join('\n');
const strayCalls = (run) => run.events.filter((e) => e.type === 'unmatched-read' || e.type === 'unmatched-write');
const unansweredReads = (run) => new Set(run.events.filter((e) => e.type === 'unmatched-read').map((e) => e.detail));
const refusals = (run) => new Set(run.events.filter((e) => e.type === 'denied').map((e) => e.detail));

/**
 * Compares two runs of one request: `a` from the old code, `b` from the new.
 * A run is { status, headers, body: Buffer, effects: string[], events: [{ type, detail }], error?, timedOut? }.
 * Answers { outcome: 'same' | 'different' | 'not-compared', kinds, details: [{ kind, path, old, new }] }, at most 20 details.
 */
export function compareRuns(a, b) {
  const kinds = new Set();
  const details = [];
  const add = (kind, path, old, now) => {
    kinds.add(kind);
    if (details.length < KEPT) details.push({ kind, path, old, new: now });
  };
  // A write one version would make and the other would not is a difference whatever else happened: a double charge is caught even
  // when the new code then fails on a call the recording cannot answer, or reaches for something a ghost may not use.
  const before = writes(a.effects);
  const after = writes(b.effects);
  if (before !== after) add('effects', 'writes', before || 'none', after || 'none');
  // Something a ghost may not use (a program, a connection no recording answers) is refused, so what it would have answered is unknown,
  // and nothing after it can be compared. Reached for by the new code alone, on the same request in the same world, it is a difference
  // in itself: left "not compared", a rare path could switch on with it unseen. Reached for by both versions, or by the old one alone
  // (while the new version serves, the old ghost is the one not checked against reality), the request is not compared, unless the
  // writes already differ.
  const oldRefused = refusals(a);
  const newRefused = refusals(b);
  if (oldRefused.size || newRefused.size) {
    const added = [...newRefused].filter((detail) => !oldRefused.has(detail));
    for (const detail of added) add('refused', 'io', 'none', detail);
    if (added.length) add('refused', 'answer', 'compared', `unknown: the ghost refused ${added[0]}, so what the new code did after it is not compared`);
    if (kinds.size) return { outcome: 'different', kinds: [...kinds], details };
    const denied = [...a.events, ...b.events].filter((e) => e.type === 'denied');
    return { outcome: 'not-compared', kinds: ['unsupported'], details: denied.slice(0, 5).map((e) => ({ kind: 'unsupported', path: 'io', old: '', new: e.detail })) };
  }
  // A write the recording cannot answer was never made, and its answer is unknown (a version's new code path writes what the old one
  // never did, say), so whatever its ghost did after it, a status, a body or a crash, came from the replay, not from the version. The
  // write itself is the difference.
  const unanswerable = [...a.events, ...b.events].find((e) => e.type === 'unmatched-write');
  if (kinds.size && unanswerable) {
    add('effects', 'answer', 'compared', `unknown: the recording cannot answer ${unanswerable.detail}, so what the ghost did after it is not compared`);
    return { outcome: 'different', kinds: [...kinds], details };
  }
  // A read the recording cannot answer, made by one ghost only: its answer is unknown, so nothing that ghost did after it can be
  // compared. Unless the writes already differ, the request is not compared.
  const oldReads = unansweredReads(a);
  const newReads = unansweredReads(b);
  const unanswered = [
    ...[...newReads].filter((d) => !oldReads.has(d)).map((d) => ({ kind: 'unanswered', path: 'calls', old: 'none', new: d })),
    ...[...oldReads].filter((d) => !newReads.has(d)).map((d) => ({ kind: 'unanswered', path: 'calls', old: d, new: 'none' })),
  ];
  if (unanswered.length) {
    if (kinds.size) return { outcome: 'different', kinds: [...kinds], details };
    return { outcome: 'not-compared', kinds: ['unanswered'], details: unanswered.slice(0, 5) };
  }
  if (a.error || b.error) {
    // Neither ghost answered in time, so what either version would have answered is unknown: the request is not compared, unless the
    // writes already differ. Two equal crashes are compared, and are the same: both versions failed alike on the same request.
    if (a.timedOut && b.timedOut) {
      if (kinds.size) return { outcome: 'different', kinds: [...kinds], details };
      return { outcome: 'not-compared', kinds: ['no-answer'], details: [{ kind: 'no-answer', path: 'result', old: a.error, new: b.error }] };
    }
    if (!a.error) add(b.timedOut ? 'hang' : 'crash', 'result', `status ${a.status}`, b.error);
    else if (!b.error) add('fixed', 'result', a.error, `status ${b.status}`);
    else if (a.error !== b.error) add('changed-error', 'result', a.error, b.error);
  } else {
    if (a.status !== b.status) {
      const kind = b.status >= 500 && a.status < 500 ? 'crash' : refused(a.status) && succeeded(b.status) ? 'door-opened' : 'status';
      add(kind, 'status', String(a.status), String(b.status));
    }
    const names = [...new Set([...Object.keys(a.headers), ...Object.keys(b.headers)])].filter((h) => !VOLATILE_HEADERS.has(h)).sort();
    for (const name of names) {
      const x = a.headers[name] === undefined ? 'missing' : String(a.headers[name]);
      const y = b.headers[name] === undefined ? 'missing' : String(b.headers[name]);
      if (x !== y) add('headers', `header ${name}`, x, y);
    }
    const x = readBody(a.body, a.headers);
    const y = readBody(b.body, b.headers);
    if (x.kind !== y.kind) add('body', 'body', x.kind, y.kind);
    else if (x.kind === 'bytes') {
      if (x.digest !== y.digest) add('body', 'body', `sha256 ${x.digest.slice(0, 16)}`, `sha256 ${y.digest.slice(0, 16)}`);
    } else {
      for (const d of diffValues(x.value, y.value)) add('body', d.path, d.old, d.new);
    }
  }
  return kinds.size ? { outcome: 'different', kinds: [...kinds], details } : { outcome: 'same', kinds: [], details: [] };
}

const matches = (pattern, path) => (pattern.endsWith('*') ? path.startsWith(pattern.slice(0, -1)) : pattern === path);

/**
 * Marks a difference the team said is meant as 'stated'. The `allow` rules for this route (or for '*') must cover every detail: by
 * naming the detail's kind, or, for kinds other than crash, hang, door-opened and effects, by matching its path ('$.user*' matches by
 * prefix). A difference with more details than were kept can only be stated by naming every one of its kinds.
 */
export function explain(verdict, route, allow) {
  if (verdict.outcome !== 'different' || !allow?.length) return verdict;
  const rules = allow.filter((rule) => rule.route === '*' || rule.route === route);
  if (!rules.length) return verdict;
  const byKind = (kind) => rules.some((rule) => rule.kinds?.includes(kind));
  const covered = (detail) => byKind(detail.kind) || (!SERIOUS.has(detail.kind) && rules.some((rule) => rule.paths?.some((p) => matches(p, detail.path))));
  const complete = verdict.details.length < KEPT;
  const stated = verdict.kinds.every(byKind) || (complete && verdict.details.every(covered));
  return stated ? { ...verdict, outcome: 'stated' } : verdict;
}

/**
 * Whether the ghost of the version that served (`who`: 'old' or 'candidate') reproduced the real run: the same status and the same
 * writes, nothing refused, and no call the real run did not make. Bodies may differ (real time, real randomness); the two ghosts are
 * compared with each other for that. Answers null when faithful, or the reason it is not.
 */
export function faithful(real, ghost, who = 'old') {
  const code = who === 'candidate' ? 'new' : 'old';
  if (ghost.error) return `the ${code} code's ghost failed: ${ghost.error}`;
  const denied = ghost.events.find((e) => e.type === 'denied');
  if (denied) return `the ghost needed something it may not use: ${denied.detail}`;
  const stray = strayCalls(ghost);
  if (stray.length) return `the ${code} code's ghost made a call the real run did not: ${stray[0].detail}`;
  if (real.status !== ghost.status) return `the ${code} code's ghost answered ${ghost.status}, the real run ${real.status}`;
  if (recordedShapes(real.effects) !== recordedShapes(ghost.effects)) return `the ${code} code's ghost wrote other things than the real run`;
  return null;
}
