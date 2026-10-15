import { diffValues, readBody } from './serialize.js';
import { digest, shapeOf } from './keys.js';

export const VOLATILE_HEADERS = new Set(['date', 'etag', 'last-modified', 'age', 'expires', 'content-length', 'connection', 'keep-alive', 'transfer-encoding', 'x-request-id', 'x-response-time', 'server-timing']);

function cookieShape(line) {
  const [pair = '', ...attributes] = String(line).split(';').map((part) => part.trim());
  const at = pair.indexOf('=');
  const name = at < 0 ? pair : pair.slice(0, at).trim();
  const value = at < 0 ? '' : pair.slice(at + 1).trim();
  const opaque = value.length >= 16 && /[A-Za-z]/.test(value) && /\d/.test(value);
  const attrs = attributes.filter(Boolean).map((part) => {
    const eq = part.indexOf('=');
    return eq < 0 ? part.toLowerCase() : `${part.slice(0, eq).trim().toLowerCase()}=${part.slice(eq + 1).trim()}`;
  }).sort();
  return [`${name}=${opaque ? '(a token)' : value}`, ...attrs].join('; ');
}

const cookiesOf = (headers) => [headers?.['set-cookie'] ?? []].flat().map(cookieShape).sort().join('\n');

const SERIOUS = new Set(['crash', 'hang', 'door-opened', 'effects', 'refused']);
const KEPT = 20;

const refused = (status) => status === 401 || status === 403 || status === 404 || status === 405 || (status >= 300 && status < 400);
const succeeded = (status) => status >= 200 && status < 300;
const MANY = 'more different writes than a ghost counts';

function tally(run) {
  if (run.writes) return new Map(run.writes);
  const out = new Map();
  for (const key of run.effects ?? []) out.set(key, (out.get(key) ?? 0) + 1);
  return out;
}

function shapes(run) {
  const out = new Map();
  for (const [key, n] of tally(run)) if (!key.startsWith('FS ')) out.set(shapeOf(key), (out.get(shapeOf(key)) ?? 0) + n);
  return out;
}

const sameTally = (x, y) => x.size === y.size && [...x].every(([key, n]) => y.get(key) === n);

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
const silent = (run) => Boolean(run.timedOut || run.gone);
const measured = (run) => run.large ?? { size: run.body.length, digest: digest(run.body) };
const sized = (body) => `${body.size} bytes, sha256 ${body.digest.slice(0, 16)}`;

export function compareRuns(a, b) {
  const kinds = new Set();
  const details = [];
  const add = (kind, path, old, now) => {
    kinds.add(kind);
    if (details.length < KEPT) details.push({ kind, path, old, new: now });
  };
  const uncounted = Boolean(a.tooMany && b.tooMany);
  if (!uncounted && (a.tooMany || b.tooMany)) {
    add('effects', 'writes', a.tooMany ? MANY : `${tally(a).size} different writes`, b.tooMany ? MANY : `${tally(b).size} different writes`);
  } else if (!uncounted) {
    const writes = differing(tally(a), tally(b));
    if (writes) add('effects', 'writes', writes.old, writes.new);
  }
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
  const unanswerable = [...a.events, ...b.events].find((e) => e.type === 'unmatched-write');
  if (kinds.size && unanswerable) {
    add('effects', 'answer', 'compared', `unknown: the recording cannot answer ${unanswerable.detail}, so what the ghost did after it is not compared`);
    return { outcome: 'different', kinds: [...kinds], details };
  }
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
  if ((a.after ?? null) !== (b.after ?? null)) add('crash', 'after the answer', a.after ?? 'none', b.after ?? 'none');
  if (a.error || b.error) {
    if (silent(a) && silent(b)) {
      if (kinds.size) return { outcome: 'different', kinds: [...kinds], details };
      return { outcome: 'not-compared', kinds: ['no-answer'], details: [{ kind: 'no-answer', path: 'result', old: a.error, new: b.error }] };
    }
    if (!a.error) add(b.timedOut ? 'hang' : 'crash', 'result', `status ${a.status}`, b.error);
    else if (!b.error) add('fixed', 'result', a.error, `status ${b.status}`);
    else if (b.timedOut && !a.timedOut) add('hang', 'result', a.error, b.error);
    else if (a.error !== b.error) add('changed-error', 'result', a.error, b.error);
  } else {
    if (a.status !== b.status) {
      const kind = b.status >= 500 && a.status < 500 ? 'crash' : refused(a.status) && succeeded(b.status) ? 'door-opened' : 'status';
      add(kind, 'status', String(a.status), String(b.status));
    }
    const names = [...new Set([...Object.keys(a.headers), ...Object.keys(b.headers)])].filter((h) => !VOLATILE_HEADERS.has(h)).sort();
    for (const name of names) {
      if (name === 'set-cookie') {
        const [x, y] = [cookiesOf(a.headers), cookiesOf(b.headers)];
        if (x !== y) add('headers', 'header set-cookie', x || 'missing', y || 'missing');
        continue;
      }
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
