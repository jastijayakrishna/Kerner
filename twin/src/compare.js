import { diffValues, readBody } from './serialize.js';

/** Headers that change on every answer and say nothing about behaviour. */
export const VOLATILE_HEADERS = new Set(['date', 'etag', 'last-modified', 'age', 'expires', 'content-length', 'connection', 'keep-alive', 'transfer-encoding', 'x-request-id', 'x-response-time', 'server-timing', 'set-cookie']);

const refused = (status) => status === 401 || status === 403 || status === 404 || status === 405 || (status >= 300 && status < 400);
const succeeded = (status) => status >= 200 && status < 300;
const writes = (list) => [...(list ?? [])].sort().join('\n');
const strayCalls = (run) => run.events.filter((e) => e.type === 'unmatched-read' || e.type === 'unmatched-write');

/**
 * Compares two runs of one request: `a` from the old code, `b` from the new.
 * A run is { status, headers, body: Buffer, effects: string[], events: [{ type, detail }], error?, timedOut? }.
 * Answers { outcome: 'same' | 'different' | 'not-compared', kinds, details: [{ path, old, new }] }.
 */
export function compareRuns(a, b) {
  const denied = [...a.events, ...b.events].filter((e) => e.type === 'denied');
  if (denied.length) return { outcome: 'not-compared', kinds: ['unsupported'], details: denied.slice(0, 5).map((e) => ({ path: 'io', old: '', new: e.detail })) };
  const kinds = new Set();
  const details = [];
  const add = (kind, path, old, now) => {
    kinds.add(kind);
    if (details.length < 20) details.push({ path, old, new: now });
  };
  if (a.error || b.error) {
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
  const before = writes(a.effects);
  const after = writes(b.effects);
  if (before !== after) add('effects', 'writes', before || 'none', after || 'none');
  if (strayCalls(b).length > strayCalls(a).length) add('new-calls', 'calls', strayCalls(a).map((e) => e.detail).join(', ') || 'none', strayCalls(b).map((e) => e.detail).join(', '));
  return kinds.size ? { outcome: 'different', kinds: [...kinds], details } : { outcome: 'same', kinds: [], details: [] };
}

/**
 * Whether the old code's ghost reproduced the real run: the same status and the same writes, nothing refused, and no call the real
 * run did not make. Bodies may differ (real time, real randomness); the two ghosts are compared with each other for that.
 * Answers null when faithful, or the reason it is not.
 */
export function faithful(real, ghost) {
  if (ghost.error) return `the old code's ghost failed: ${ghost.error}`;
  const denied = ghost.events.find((e) => e.type === 'denied');
  if (denied) return `the ghost needed something it may not use: ${denied.detail}`;
  const stray = strayCalls(ghost);
  if (stray.length) return `the old code's ghost made a call the real run did not: ${stray[0].detail}`;
  if (real.status !== ghost.status) return `the old code's ghost answered ${ghost.status}, the real run ${real.status}`;
  if (writes(real.effects) !== writes(ghost.effects)) return 'the old code\'s ghost wrote other things than the real run';
  return null;
}
