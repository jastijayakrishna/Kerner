import { AsyncLocalStorage } from 'node:async_hooks';
import { randomInt } from 'node:crypto';
import { exactBytes } from './serialize.js';
import { routeOf } from './route.js';

export const current = new AsyncLocalStorage();

const BUDGET_BODIES = 8;
export const CALL_BYTES = 1024;
const budgetOf = (maxBodyBytes) => BUDGET_BODIES * maxBodyBytes;

export function spend(record, bytes) {
  record.spent += bytes;
  if (record.spent > record.budget) record.problem ??= `copies of more than ${record.budget} bytes in all (${BUDGET_BODIES} times maxBodyBytes; a call counts at least ${CALL_BYTES} bytes)`;
}

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

function keep(emitter, record) {
  const emit = emitter.emit;
  emitter.emit = function (...args) {
    return current.run(record, () => emit.apply(this, args));
  };
}

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

export function loadRecord(maxBodyBytes = 1 << 20) {
  return {
    start: Date.now(), seed: randomInt(2 ** 31), io: [], pending: new Set(), serving: 'old',
    incoming: [], outgoing: [], sizes: { incoming: 0, outgoing: 0 }, head: null, problem: null, done: false, spent: 0, budget: budgetOf(maxBodyBytes), maxBodyBytes,
  };
}

export async function settleLoad(record) {
  await new Promise((resolve) => setImmediate(resolve));
  if (record.pending.size) await Promise.race([Promise.allSettled([...record.pending]), new Promise((resolve) => setTimeout(resolve, 100))]);
  record.done = true;
  return { start: record.start, seed: record.seed, io: [...record.io] };
}
