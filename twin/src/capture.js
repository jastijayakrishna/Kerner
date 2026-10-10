import { AsyncLocalStorage } from 'node:async_hooks';
import { randomInt } from 'node:crypto';

/** The record of the sampled request being served, so the calls the app makes while serving it are written down with it. */
export const current = new AsyncLocalStorage();

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
 * Wraps a Node request listener. A request that is not sampled goes straight to the listener. A sampled request is served exactly
 * as before while Kern keeps a copy of what came in, what went out and every outgoing call; `deliver` gets a promise of that copy
 * once the answer has gone out.
 */
export function wrapListener(listener, { sample, maxBodyBytes, deliver, onError }) {
  return function kernTwin(req, res) {
    let record = null;
    try {
      if (sample()) record = begin(req, res, maxBodyBytes, deliver);
    } catch (error) {
      onError(error);
      record = null;
    }
    if (!record) return listener(req, res);
    return current.run(record, () => listener(req, res));
  };
}

function begin(req, res, maxBodyBytes, deliver) {
  const record = {
    start: Date.now(), seed: randomInt(2 ** 31), io: [], pending: new Set(),
    incoming: [], outgoing: [], sizes: { incoming: 0, outgoing: 0 }, head: null, problem: null, done: false,
  };
  const copy = (side, chunk, encoding) => {
    try {
      if (record.problem || chunk == null) return;
      const bytes = bytesOf(chunk, encoding);
      if (!bytes) return;
      record.sizes[side] += bytes.length;
      if (record.sizes[side] > maxBodyBytes) record.problem = `a body over ${maxBodyBytes} bytes`;
      else record[side].push(Buffer.from(bytes));
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
  res.once('finish', () => deliver(finish(req, res, record)));
  return record;
}

async function finish(req, res, record) {
  // Calls the app starts right after answering still belong to this request: give them, and copies still being read, a moment.
  await new Promise((resolve) => setImmediate(resolve));
  if (record.pending.size) await Promise.race([Promise.allSettled([...record.pending]), new Promise((resolve) => setTimeout(resolve, 100))]);
  record.done = true;
  if (record.problem) return { skipped: record.problem };
  if (record.pending.size) return { skipped: 'an outgoing call was still running' };
  const headers = lower(req.headers);
  for (const name of HOP) delete headers[name];
  return {
    start: record.start,
    seed: record.seed,
    request: { method: req.method, url: req.url, headers, body: Buffer.concat(record.incoming).toString('base64') },
    response: { status: res.statusCode, headers: { ...lower(res.getHeaders()), ...lower(record.head) }, body: Buffer.concat(record.outgoing) },
    io: record.io,
    effects: record.io.filter((call) => call.write).map((call) => call.key),
  };
}
