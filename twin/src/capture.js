import { AsyncLocalStorage } from 'node:async_hooks';
import { randomInt } from 'node:crypto';
import { exactBytes } from './serialize.js';
import { routeOf } from './route.js';

/** The record of the sampled request being served, so the calls the app makes while serving it are written down with it. */
export const current = new AsyncLocalStorage();

// What one request may copy in all: its body, its answer and every call it makes, each call counting at least CALL_BYTES beside its bodies
// (what keeping it costs). Each body has its own cap, maxBodyBytes; without this one, a request that fans out to many calls, however
// small, could hold any amount of the app's memory (R7).
const BUDGET_BODIES = 8;
export const CALL_BYTES = 1024;
const budgetOf = (maxBodyBytes) => BUDGET_BODIES * maxBodyBytes;

/** Counts what a request's copies take; past its budget, the request is skipped. */
export function spend(record, bytes) {
  record.spent += bytes;
  if (record.spent > record.budget) record.problem ??= `copies of more than ${record.budget} bytes in all (${BUDGET_BODIES} times maxBodyBytes; a call counts at least ${CALL_BYTES} bytes)`;
}

// Headers about the connection, not the request; the ghost's own connection sets its own.
const HOP = ['connection', 'keep-alive', 'proxy-connection', 'transfer-encoding', 'te', 'trailer', 'upgrade', 'content-length'];

function lower(headers) {
  const out = {};
  for (const [name, value] of Object.entries(headers ?? {})) out[name.toLowerCase()] = Array.isArray(value) ? value.map(String) : String(value);
  return out;
}

function bytesOf(chunk, encoding) {
  if (Buffer.isBuffer(chunk)) return chunk;
  if (typeof chunk === 'string') return Buffer.from(chunk, typeof encoding === 'string' ? encoding : 'utf8');
  if (chunk instanceof Uint8Array) return Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  return null;
}

/**
 * Runs every event of a sampled request, and of its answer, inside the request's record. Node runs a body's 'data' and 'end' events on
 * the connection's own context (measured on Node 22). So without this, a body parser's listeners, and all they start (the route's
 * handler and its calls), would run outside the record, and those calls would go unrecorded.
 */
function keep(emitter, record) {
  const emit = emitter.emit;
  emitter.emit = function (...args) {
    return current.run(record, () => emit.apply(this, args));
  };
}

/**
 * Wraps a Node request listener. A request that is not sampled goes straight to the listener. A sampled request is served exactly
 * as before while Kern keeps a copy of what came in, what went out and every outgoing call. `deliver` gets a promise of that copy
 * once the answer has gone out, or of a skip when the request was aborted or could not be copied. `describe(req)` may name the
 * request's route and customer segment: { route, segment }.
 */
export function wrapListener(listener, { sample, maxBodyBytes, deliver, onError, describe = null }) {
  return function kernTwin(req, res) {
    let record = null;
    try {
      if (sample()) record = begin(req, res, maxBodyBytes, deliver, describe);
    } catch (error) {
      onError(error);
      record = null;
    }
    if (!record) return listener(req, res);
    return current.run(record, () => listener(req, res));
  };
}

function begin(req, res, maxBodyBytes, deliver, describe) {
  const record = {
    // The method and url as the request arrived: an app that mounts sub-routers (Express app.use('/v1', router)) rewrites req.url while
    // routing and may not restore it, so these are read now, before the app runs, not when the answer is recorded.
    method: req.method, url: req.url,
    start: Date.now(), seed: randomInt(2 ** 31), io: [], pending: new Set(), serving: 'old',
    incoming: [], outgoing: [], sizes: { incoming: 0, outgoing: 0 }, head: null, problem: null, done: false, spent: 0, budget: budgetOf(maxBodyBytes), maxBodyBytes,
  };
  const copy = (side, chunk, encoding) => {
    try {
      if (record.problem || chunk == null) return;
      const bytes = bytesOf(chunk, encoding);
      if (!bytes) return;
      record.sizes[side] += bytes.length;
      if (record.sizes[side] > maxBodyBytes) record.problem = `a body over ${maxBodyBytes} bytes`;
      else {
        record[side].push(Buffer.from(bytes));
        spend(record, bytes.length);
      }
    } catch {
      record.problem ??= 'a body Kern could not copy';
    }
  };
  // The HTTP parser hands every piece of the body to push(), however the app reads it (events, read() or async iteration).
  const push = req.push;
  req.push = function (chunk, encoding) {
    copy('incoming', chunk, encoding);
    return push.call(this, chunk, encoding);
  };
  const { writeHead, write, end } = res;
  res.writeHead = function (...args) {
    try {
      const headers = args.slice(1).find((a) => a && typeof a === 'object' && !Array.isArray(a));
      if (headers) record.head = headers;
    } catch {
      // The answer goes out unchanged either way.
    }
    return writeHead.apply(this, args);
  };
  res.write = function (chunk, encoding, callback) {
    copy('outgoing', chunk, encoding);
    return write.call(this, chunk, encoding, callback);
  };
  res.end = function (chunk, encoding, callback) {
    if (typeof chunk !== 'function') copy('outgoing', chunk, encoding);
    return end.call(this, chunk, encoding, callback);
  };
  // Every sampled request is accounted for: it ends with its answer sent ('finish'), or broken off first ('close' without 'finish').
  let ended = false;
  res.once('finish', () => {
    ended = true;
    deliver(finish(req, res, record, describe));
  });
  res.once('close', () => {
    if (ended) return;
    ended = true;
    deliver(aborted(req, record, describe));
  });
  keep(req, record);
  keep(res, record);
  return record;
}

/**
 * Where a request belongs: when it came, its route, its customer segment and the version that served it. Without naming functions a
 * request gets the route of its path and the segment 'all'; a naming function that throws or answers nothing leaves the route of the
 * path and the segment 'unknown', so the request is still counted.
 */
function where(req, record, describe) {
  let named = { segment: 'all' };
  if (describe) {
    try {
      named = describe(req) ?? {};
    } catch {
      named = {};
    }
  }
  const text = (value, max) => (typeof value === 'string' && value ? value.slice(0, max) : null);
  return {
    start: record.start,
    route: text(named.route, 200) ?? routeOf(record.method, record.url),
    segment: text(named.segment, 100) ?? 'unknown',
    serving: record.serving,
  };
}

async function finish(req, res, record, describe) {
  // Calls the app starts right after answering still belong to this request: give them, and copies still being read, a moment.
  await new Promise((resolve) => setImmediate(resolve));
  if (record.pending.size) await Promise.race([Promise.allSettled([...record.pending]), new Promise((resolve) => setTimeout(resolve, 100))]);
  record.done = true;
  const place = where(req, record, describe);
  if (record.problem) return { skipped: record.problem, ...place };
  if (record.pending.size) return { skipped: 'an outgoing call was still running', ...place };
  const headers = lower(req.headers);
  for (const name of HOP) delete headers[name];
  return {
    ...place,
    seed: record.seed,
    request: { method: record.method, url: record.url, headers, body: exactBytes(record.incoming) },
    response: { status: res.statusCode, headers: { ...lower(res.getHeaders()), ...lower(record.head) }, body: exactBytes(record.outgoing) },
    io: record.io,
    effects: record.io.filter((call) => call.write).map((call) => call.key),
  };
}

/**
 * A request the customer gave up on before its answer finished. Nothing answered it in full, so it is skipped, unless its request came in
 * whole and its calls were all copied: then both ghosts can answer it, and the coordinator looks for one thing only, a version that hangs
 * where the other answers. While the new version serves, that is the hang the customer met, and it switches serving back.
 */
async function aborted(req, record, describe) {
  await new Promise((resolve) => setImmediate(resolve));
  if (record.pending.size) await Promise.race([Promise.allSettled([...record.pending]), new Promise((resolve) => setTimeout(resolve, 100))]);
  record.done = true;
  const place = where(req, record, describe);
  if (record.problem || record.pending.size || !req.complete) return { skipped: 'the request was aborted before its answer finished', ...place };
  const headers = lower(req.headers);
  for (const name of HOP) delete headers[name];
  return {
    ...place,
    seed: record.seed,
    aborted: true,
    request: { method: record.method, url: record.url, headers, body: exactBytes(record.incoming) },
    io: record.io,
    effects: record.io.filter((call) => call.write).map((call) => call.key),
  };
}

/**
 * A record for the app's start (Phase 6): the calls the old version makes while it loads in the app's thread are written down into it
 * exactly as a sampled request's are, by the same recorders.
 */
export function loadRecord(maxBodyBytes = 1 << 20) {
  return {
    start: Date.now(), seed: randomInt(2 ** 31), io: [], pending: new Set(), serving: 'old',
    incoming: [], outgoing: [], sizes: { incoming: 0, outgoing: 0 }, head: null, problem: null, done: false, spent: 0, budget: budgetOf(maxBodyBytes), maxBodyBytes,
  };
}

/**
 * Closes a start's record once the version has loaded, as a request's record is closed once it is answered. Calls still running a
 * moment later belong to the app's background work, and are left out. Answers what the ghosts need: { start, seed, io }.
 */
export async function settleLoad(record) {
  await new Promise((resolve) => setImmediate(resolve));
  if (record.pending.size) await Promise.race([Promise.allSettled([...record.pending]), new Promise((resolve) => setTimeout(resolve, 100))]);
  record.done = true;
  return { start: record.start, seed: record.seed, io: [...record.io] };
}
