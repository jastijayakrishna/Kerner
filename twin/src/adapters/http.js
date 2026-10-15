import http from 'node:http';
import https from 'node:https';
import { syncBuiltinESMExports } from 'node:module';
import { callKey, isWrite, readAllowed } from '../keys.js';
import { exactBytes } from '../serialize.js';
import { mine, owner, turnOf, turnText, within } from '../turn.js';
import { CALL_BYTES, spend } from '../capture.js';

const PATCHED = Symbol.for('kern.twin.http');
const HOP = new Set(['connection', 'keep-alive', 'transfer-encoding', 'content-length', 'proxy-connection', 'te', 'trailer', 'upgrade']);

const bracketed = (host) => (/:.*:/.test(host) && !host.startsWith('[') ? `[${host}]` : host);

function hostHeader(headers) {
  if (!headers || typeof headers !== 'object' || Array.isArray(headers)) return null;
  const name = Object.keys(headers).find((key) => key.toLowerCase() === 'host');
  return name === undefined || headers[name] == null ? null : String(headers[name]);
}

function addressOf(scheme, args) {
  let url = null;
  let options = {};
  let callback = null;
  for (const arg of args) {
    if (typeof arg === 'function') callback = arg;
    else if (typeof arg === 'string' || arg instanceof URL) url = new URL(String(arg));
    else if (arg && typeof arg === 'object') options = { ...options, ...arg };
  }
  const parts = url ? { protocol: url.protocol, hostname: url.hostname.replace(/^\[(.*)\]$/, '$1'), port: url.port || undefined, path: `${url.pathname}${url.search}` } : {};
  const merged = { ...parts, ...options };
  const protocol = merged.protocol ?? scheme;
  const host = bracketed(String(merged.hostname || merged.host || 'localhost'));
  const defaultPort = Number(merged.defaultPort || merged.agent?.defaultPort || (protocol === 'https:' ? 443 : 80));
  const port = Number(merged.port || defaultPort);
  const bare = merged.setHost === false || Array.isArray(merged.headers);
  const authority = hostHeader(merged.headers) ?? (bare || port === defaultPort ? host : `${host}:${port}`);
  return { url: `${protocol}//${authority}${merged.path ?? '/'}`, method: String(merged.method ?? 'GET').toUpperCase(), headers: merged.headers ?? {}, options: merged, callback };
}

export function plainHeaders(headers) {
  const pairs = Array.isArray(headers) ? headers.flatMap((item, i) => (i % 2 ? [] : [[String(item), headers[i + 1]]])) : Object.entries(headers ?? {});
  const out = {};
  for (const [name, value] of pairs) {
    const lower = name.toLowerCase();
    if (value === undefined || HOP.has(lower) || lower === 'host') continue;
    out[lower] = lower in out ? [out[lower], value].flat() : value;
  }
  return out;
}

function watch(sampled, req, scheme, installedLimit) {
  const maxBodyBytes = sampled.maxBodyBytes ?? installedLimit;
  const method = String(req.method ?? 'GET').toUpperCase();
  const url = `${req.protocol ?? scheme}//${req.getHeader('host') ?? bracketed(String(req.host))}${req.path}`;
  const call = { kind: 'http', key: callKey(method, url, null), method, url, write: isWrite(method), sent: new Uint8Array(), status: 0, headers: {}, body: null, error: null };
  sampled.io.push(call);
  spend(sampled, CALL_BYTES);
  let settle;
  const tracked = new Promise((resolve) => {
    settle = resolve;
  });
  sampled.pending.add(tracked);
  tracked.then(() => sampled.pending.delete(tracked));
  const sent = [];
  let size = 0;
  const keep = (chunk, encoding) => {
    if (chunk == null || typeof chunk === 'function') return;
    let bytes;
    try {
      bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk, typeof encoding === 'string' ? encoding : 'utf8');
    } catch {
      sampled.problem ??= "an outgoing request's body could not be copied";
      return;
    }
    size += bytes.length;
    if (size > maxBodyBytes) sampled.problem ??= 'an outgoing request with a body over the limit';
    else {
      sent.push(Buffer.from(bytes));
      spend(sampled, bytes.length);
    }
  };
  const { write, end, emit } = req;
  req.write = function (chunk, encoding, callback) {
    keep(chunk, encoding);
    return write.call(this, chunk, encoding, callback);
  };
  req.end = function (chunk, encoding, callback) {
    keep(chunk, encoding);
    call.sent = exactBytes(sent);
    call.key = callKey(method, url, call.sent);
    return end.call(this, chunk, encoding, callback);
  };
  req.emit = function (name, ...args) {
    if (name === 'response') hear(args[0]);
    else if (name === 'error' && !call.status) {
      call.error = String(args[0]?.message ?? args[0]);
      settle();
    } else if (name === 'close' && !call.status) settle();
    return emit.call(this, name, ...args);
  };
  function hear(res) {
    call.status = res.statusCode;
    call.headers = { ...res.headers };
    const parts = [];
    let total = 0;
    const push = res.push;
    res.push = function (chunk, encoding) {
      try {
        if (chunk === null) {
          call.body = exactBytes(parts);
          settle();
        } else {
          const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk, encoding);
          total += bytes.length;
          if (total > maxBodyBytes) sampled.problem ??= 'an outgoing call answered with a body over the limit';
          else {
            parts.push(Buffer.from(bytes));
            spend(sampled, bytes.length);
          }
        }
      } catch {
        sampled.problem ??= "an outgoing call's answer could not be copied";
      }
      return push.call(this, chunk, encoding);
    };
    res.once('close', () => settle());
  }
}

function record({ current, maxBodyBytes }) {
  for (const [module, scheme] of [[http, 'http:'], [https, 'https:']]) {
    if (module[PATCHED]) continue;
    const request = module.request;
    module.request = function kernRequest(...args) {
      const req = request.apply(this, args);
      const sampled = current.getStore();
      if (sampled && !sampled.done) watch(sampled, req, scheme, maxBodyBytes);
      return req;
    };
    module.get = function kernGet(...args) {
      const req = module.request(...args);
      req.end();
      return req;
    };
    module[PATCHED] = true;
  }
  syncBuiltinESMExports();
}

async function prepare({ take, wrote, note, ask, realReads }) {
  const server = http.createServer((req, res) => within(turnOf(String(req.headers['x-kern-turn'] ?? '')), async () => {
    if (!mine()) return;
    const url = String(req.headers['x-kern-original-url'] ?? '');
    const method = String(req.headers['x-kern-original-method'] ?? req.method).toUpperCase();
    const parts = [];
    for await (const part of req) parts.push(part);
    const body = Buffer.concat(parts);
    const key = callKey(method, url, body);
    const write = isWrite(method);
    if (write) wrote(key);
    let answer = take(key);
    if (!answer && !write && readAllowed(realReads, method, url)) {
      const real = await ask({ type: 'real-read', method, url, headers: plainHeaders(Object.fromEntries(Object.entries(req.headers).filter(([name]) => !name.startsWith('x-kern-')))) });
      if (!real.error) {
        note('real-read', `${method} ${url}`);
        answer = real;
      }
    }
    if (!answer) note(write ? 'unmatched-write' : 'unmatched-read', `${method} ${url}`);
    if (!answer || answer.error) {
      res.socket.destroy();
      return;
    }
    const headers = Object.fromEntries(Object.entries(answer.headers ?? {}).filter(([name]) => !HOP.has(name)));
    res.writeHead(answer.status, headers);
    res.end(answer.body ? Buffer.from(answer.body.buffer, answer.body.byteOffset, answer.body.byteLength) : undefined);
  }));
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { ports: [server.address().port], replayPort: server.address().port };
}

function replay({ replayPort }) {
  const realRequest = http.request;
  for (const [module, scheme] of [[http, 'http:'], [https, 'https:']]) {
    module.request = function ghostRequest(...args) {
      const target = addressOf(scheme, args);
      const headers = { ...plainHeaders(target.headers), 'x-kern-original-url': target.url, 'x-kern-original-method': target.method, 'x-kern-turn': turnText(owner()) };
      const options = { host: '127.0.0.1', port: replayPort, method: target.method, path: '/', headers, agent: false, signal: target.options.signal, timeout: target.options.timeout };
      return target.callback ? realRequest(options, target.callback) : realRequest(options);
    };
    module.get = function ghostGet(...args) {
      const req = module.request(...args);
      req.end();
      return req;
    };
  }
  syncBuiltinESMExports();
}

export default { kind: 'http', record, prepare, replay };
