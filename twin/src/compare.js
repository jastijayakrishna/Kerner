import { diffValues, readBody } from './serialize.js';
import { digest, shapeOf } from './keys.js';

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
const MANY = 'more different writes than a ghost counts';

/**
 * Every write of a run by key, with how often it was made. A ghost counts all of its writes (`writes`, [key, how often] pairs); a run
 * without counts (the real run, whose record lists every write, or a run kept in an older case) is counted from its list.
 */
function tally(run) {
  if (run.writes) return new Map(run.writes);
  const out = new Map();
  for (const key of run.effects ?? []) out.set(key, (out.get(key) ?? 0) + 1);
  return out;
}

// The real run records calls (fetch, http, Postgres, Redis), not file writes, and its payloads carry the real clock: what it wrote is
// compared with the serving ghost by shape, every write counted, and file writes are compared between the two ghosts only.
function shapes(run) {
  const out = new Map();
  for (const [key, n] of tally(run)) if (!key.startsWith('FS ')) out.set(shapeOf(key), (out.get(shapeOf(key)) ?? 0) + n);
  return out;
}

const sameTally = (x, y) => x.size === y.size && [...x].every(([key, n]) => y.get(key) === n);

/** The writes two tallies differ in, as each run made them, a key with how often: at most 200 a side, so a person sees them all. */
function differing(x, y) {
  const keys = [...new Set([...x.keys(), ...y.keys()])].filter((key) => x.get(key) !== y.get(key)).sort();
  if (!keys.length) return null;
  const side = (made) => {
    const list = keys.filter((key) => made.has(key)).map((key) => (made.get(key) > 1 ? `${key} (${made.get(key)} times)` : key));
    if (!list.length) return 'none';
    return `${list.slice(0, 200).join('\n')}${list.length > 200 ? `\nand ${list.length - 200} more` : ''}`;
  };
  return { old: side(x), new: side(y) };
}

const strayCalls = (run) => run.events.filter((e) => e.type === 'unmatched-read' || e.type === 'unmatched-write');
const unansweredReads = (run) => new Set(run.events.filter((e) => e.type === 'unmatched-read').map((e) => e.detail));
const refusals = (run) => new Set(run.events.filter((e) => e.type === 'denied').map((e) => e.detail));
// A ghost that gave this request no answer of its own: it ran out of time, or it had stopped before the request came (`gone`).
const silent = (run) => Boolean(run.timedOut || run.gone);
// An answer over maxBodyBytes crossed from its ghost only as its size and sha256 (`large`); a body that crossed whole is measured the same
// way when it is compared with one.
const measured = (run) => run.large ?? { size: run.body.length, digest: digest(run.body) };
const sized = (body) => `${body.size} bytes, sha256 ${body.digest.slice(0, 16)}`;

/**
 * Compares two runs of one request: `a` from the old code, `b` from the new.
 * A run is { status, headers, body: Buffer, large?: { size, digest }, effects: string[], writes?: [key, count][], tooMany?, events: [{ type, detail }],
 * error?, timedOut?, gone? }.
 * Answers { outcome: 'same' | 'different' | 'not-compared', kinds, details: [{ kind, path, old, new }] }, at most 20 details.
 */
export function compareRuns(a, b) {
  const kinds = new Set();
  const details = [];
  const add = (kind, path, old, now) => {
    kinds.add(kind);
    if (details.length < KEPT) details.push({ kind, path, old, new: now });
  };
  // A write one version would make and the other would not, or would make more often, is a difference whatever else happened: a double
  // charge is caught even when the new code then fails on a call the recording cannot answer, or reaches for something a ghost may not
  // use. Every write counts, by key. A ghost that made more different writes than it counts (`tooMany`) cannot be compared in full: when
  // only one did, the two certainly differ; when both did, the request is not compared, unless something else differs.
  const uncounted = Boolean(a.tooMany && b.tooMany);
  if (!uncounted && (a.tooMany || b.tooMany)) {
    add('effects', 'writes', a.tooMany ? MANY : `${tally(a).size} different writes`, b.tooMany ? MANY : `${tally(b).size} different writes`);
  } else if (!uncounted) {
    const writes = differing(tally(a), tally(b));
    if (writes) add('effects', 'writes', writes.old, writes.new);
  }
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
  // A version that failed after it had answered (an uncaught error in work the request left running) would end a production process
  // that has no handler for it: a difference, whatever its answer was. Both failing alike behave alike.
  if ((a.after ?? null) !== (b.after ?? null)) add('crash', 'after the answer', a.after ?? 'none', b.after ?? 'none');
  if (a.error || b.error) {
    // Neither ghost answered (each ran out of time, or had stopped before the request came), so what either version would have answered
    // is unknown: the request is not compared, unless the writes already differ. Two equal crashes are compared, and are the same: both
    // versions failed alike on the same request.
    if (silent(a) && silent(b)) {
      if (kinds.size) return { outcome: 'different', kinds: [...kinds], details };
      return { outcome: 'not-compared', kinds: ['no-answer'], details: [{ kind: 'no-answer', path: 'result', old: a.error, new: b.error }] };
    }
    if (!a.error) add(b.timedOut ? 'hang' : 'crash', 'result', `status ${a.status}`, b.error);
    else if (!b.error) add('fixed', 'result', a.error, `status ${b.status}`);
    // A request the old version failed on fast and the new one now hangs on is a hang: worse for a customer, so only a rule naming it may
    // state it.
    else if (b.timedOut && !a.timedOut) add('hang', 'result', a.error, b.error);
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
    if (a.large || b.large) {
      const x = measured(a);
      const y = measured(b);
      if (x.size !== y.size || x.digest !== y.digest) add('body', 'body', sized(x), sized(y));
    } else {
      const x = readBody(a.body, a.headers);
      const y = readBody(b.body, b.headers);
      if (x.kind !== y.kind) add('body', 'body', x.kind, y.kind);
      else if (x.kind === 'bytes') {
        if (x.digest !== y.digest) add('body', 'body', `sha256 ${x.digest.slice(0, 16)}`, `sha256 ${y.digest.slice(0, 16)}`);
      } else {
        for (const d of diffValues(x.value, y.value)) add('body', d.path, d.old, d.new);
      }
    }
  }
  if (kinds.size) return { outcome: 'different', kinds: [...kinds], details };
  if (uncounted) return { outcome: 'not-compared', kinds: ['too-many-writes'], details: [{ kind: 'too-many-writes', path: 'writes', old: MANY, new: MANY }] };
  return { outcome: 'same', kinds: [], details: [] };
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
  if (ghost.tooMany) return `the ${code} code's ghost made ${MANY}, so its writes cannot be checked against the real run`;
  if (!sameTally(shapes(real), shapes(ghost))) return `the ${code} code's ghost wrote other things than the real run`;
  return null;
}
