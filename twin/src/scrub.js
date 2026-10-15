import { createHmac, randomBytes } from 'node:crypto';
import { callKey, pgKey, redisKey } from './keys.js';
import { decodeBody, exactBytes, jsonText } from './serialize.js';

export const DEFAULT_SCRUB = {
  headers: ['authorization', 'proxy-authorization', 'cookie', 'set-cookie', 'x-api-key', 'x-auth-token', 'x-csrf-token'],
  keys: ['password', 'passwd', 'secret', 'token', 'apikey', 'api_key', 'accesstoken', 'access_token', 'refreshtoken', 'refresh_token', 'ssn',
    'cardnumber', 'card_number', 'card', 'cvv', 'cvc', 'iban'],
};

const EMAIL = /[A-Za-z0-9._%+-]{1,64}@[A-Za-z0-9.-]{1,253}\.[A-Za-z]{2,63}/g;
const CARD = /\b\d(?:[ -]?\d){12,18}\b/g;
const BEARER = /\b(Bearer|Basic|Token)\s+([A-Za-z0-9._~+/=-]{3,})/gi;
const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi;
const IS_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const RUN = /\b[A-Za-z0-9_-]{32,}\b/g;
const FORM = /^[^=&\s]+=[^&\s]*(?:&[^=&\s]+=[^&\s]*)*$/;
const LOWER = 'abcdefghijklmnopqrstuvwxyz';
const UPPER = LOWER.toUpperCase();

function luhnDigit(body) {
  let sum = 0;
  [...body].reverse().forEach((d, i) => {
    let n = Number(d);
    if (i % 2 === 0) n = n * 2 > 9 ? n * 2 - 9 : n * 2;
    sum += n;
  });
  return String((10 - (sum % 10)) % 10);
}

const luhnValid = (digits) => digits.length >= 13 && luhnDigit(digits.slice(0, -1)) === digits.at(-1);

export function scrubber(rules = {}) {
  const secret = randomBytes(32);
  const headers = new Set([...DEFAULT_SCRUB.headers, ...(rules.headers ?? [])].map((name) => name.toLowerCase()));
  const keys = new Set([...DEFAULT_SCRUB.keys, ...(rules.keys ?? [])].map((name) => name.toLowerCase()));
  const digest = (value) => createHmac('sha256', secret).update(String(value)).digest();
  const shaped = (value) => {
    const plain = String(value);
    const bytes = digest(plain);
    const hex = /[a-f]/i.test(plain) && /^[0-9a-f-]+$/i.test(plain);
    return [...plain].map((c, i) => {
      const b = bytes[i % bytes.length];
      if (c >= 'a' && c <= 'z') return LOWER[b % (hex ? 6 : 26)];
      if (c >= 'A' && c <= 'Z') return UPPER[b % (hex ? 6 : 26)];
      if (c >= '0' && c <= '9') return String(b % 10);
      return c;
    }).join('');
  };
  const email = (value) => `user-${digest(value.toLowerCase()).toString('hex').slice(0, 8)}@example.com`;
  const card = (value) => {
    const digits = value.replace(/\D/g, '');
    if (!luhnValid(digits)) return value;
    const body = `4${[...digest(digits)].map((b) => b % 10).join('').padEnd(digits.length, '0').slice(0, digits.length - 2)}`;
    const full = body + luhnDigit(body);
    let at = 0;
    return value.replace(/\d/g, () => full[at++]);
  };
  const uuid = (value) => {
    const hex = digest(value.toLowerCase()).toString('hex');
    let at = 0;
    const out = value.toLowerCase().replace(/[0-9a-f]/g, (c, i) => (i === 14 || i === 19 ? c : hex[at++]));
    return /[A-F]/.test(value) ? out.toUpperCase() : out;
  };
  const text = (value) => {
    let out = String(value).replace(BEARER, (all, kind, token) => `${kind} ${shaped(token)}`);
    if (out.includes('@')) out = out.replace(EMAIL, email);
    return out.replace(CARD, card).replace(UUID, uuid)
      .replace(RUN, (run) => (/\d/.test(run) && /[A-Za-z]/.test(run) && !IS_UUID.test(run) ? shaped(run) : run));
  };
  const scalar = (v, sensitive) => {
    const replaced = text(v);
    return sensitive && replaced === v ? shaped(v) : replaced;
  };
  const leaf = (v, name) => {
    const sensitive = keys.has(String(name).toLowerCase());
    if (typeof v === 'string') return scalar(v, sensitive);
    if (typeof v === 'number' && sensitive) return Number(shaped(String(v)));
    return v;
  };
  const container = (v) => v !== null && typeof v === 'object' && !(v instanceof Uint8Array) && !(v instanceof Date);
  const value = (input, name = '') => {
    if (!container(input)) return leaf(input, name);
    const frame = (source, key, slot) => ({ source, key, slot, entries: Array.isArray(source) ? null : Object.entries(source), next: 0, items: [] });
    const keep = (holder, slot, built) => (holder.entries ? holder.items.push([slot, built]) : holder.items.push(built));
    const path = new Set([input]);
    const stack = [frame(input, name, null)];
    let result;
    while (stack.length) {
      const top = stack.at(-1);
      const size = top.entries ? top.entries.length : top.source.length;
      if (top.next >= size) {
        stack.pop();
        path.delete(top.source);
        const built = top.entries ? Object.fromEntries(top.items) : top.items;
        if (stack.length) keep(stack.at(-1), top.slot, built);
        else result = built;
        continue;
      }
      const i = top.next++;
      const [slot, item] = top.entries ? top.entries[i] : [i, top.source[i]];
      const key = top.entries ? slot : top.key;
      if (!container(item)) keep(top, slot, leaf(item, key));
      else if (path.has(item)) keep(top, slot, null);
      else {
        path.add(item);
        stack.push(frame(item, key, slot));
      }
    }
    return result;
  };
  const headersOf = (list) => Object.fromEntries(Object.entries(list ?? {}).map(([name, v]) => {
    const lower = name.toLowerCase();
    const one = (item) => (headers.has(lower) ? '[scrubbed]' : lower === 'content-type' ? item : text(item));
    return [name, Array.isArray(v) ? v.map(one) : one(v)];
  }));
  const piece = (raw, scrub, query) => {
    let plain;
    try {
      plain = decodeURIComponent(query ? raw.replace(/\+/g, ' ') : raw);
    } catch {
      return scrub(raw);
    }
    const out = scrub(plain);
    if (out === plain) return raw;
    let encoded = encodeURIComponent(out);
    for (const c of new Set(raw.match(/[^%A-Za-z0-9\-_.!~*'()]/g))) encoded = encoded.replaceAll(encodeURIComponent(c), c);
    return query && raw.includes('+') ? encoded.replaceAll('%20', '+') : encoded;
  };
  const query = (raw) => raw.split('&').map((pair) => {
    const at = pair.indexOf('=');
    if (at < 0) return piece(pair, text, true);
    const name = pair.slice(0, at);
    let key = name;
    try {
      key = decodeURIComponent(name.replace(/\+/g, ' '));
    } catch {
    }
    const sensitive = keys.has(key.toLowerCase());
    return `${piece(name, text, true)}=${piece(pair.slice(at + 1), (v) => scalar(v, sensitive), true)}`;
  }).join('&');
  const url = (raw) => {
    const [, origin = '', path, search, hash = ''] = /^([A-Za-z][A-Za-z0-9+.-]*:\/\/[^/?#]*)?([^?#]*)(?:\?([^#]*))?(#[\s\S]*)?$/.exec(String(raw));
    const pieces = path.split('/').map((segment) => piece(segment, text, false)).join('/');
    return `${origin}${pieces}${search === undefined ? '' : `?${query(search)}`}${hash && text(hash)}`;
  };
  const body = (bytes, list = {}) => {
    if (!bytes || !bytes.length) return { bytes: bytes ?? new Uint8Array(), headers: list, opaque: false };
    let plain;
    try {
      plain = decodeBody(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength), list);
    } catch {
      return { bytes, headers: list, opaque: true };
    }
    const type = String(list['content-type'] ?? 'text/plain').toLowerCase();
    const out = { ...list };
    delete out['content-encoding'];
    delete out['content-length'];
    const utf8 = plain.toString('utf8');
    if (type.includes('json') || /^\s*[[{]/.test(utf8)) {
      try {
        return { bytes: exactBytes([Buffer.from(jsonText(value(JSON.parse(utf8))))]), headers: out, opaque: false };
      } catch {
      }
    }
    const readable = Buffer.from(utf8, 'utf8').equals(plain);
    if (readable && (type.startsWith('text/') || type.includes('json') || type.includes('xml') || type.includes('form-urlencoded') || type.includes('javascript'))) {
      const form = type.includes('form-urlencoded') || FORM.test(utf8);
      return { bytes: exactBytes([Buffer.from(form ? query(utf8) : text(utf8))]), headers: out, opaque: false };
    }
    return { bytes, headers: list, opaque: true };
  };
  return { text, value, headersOf, url, body };
}

export function scrubCapture(capture, rules) {
  const s = scrubber(rules);
  const opaque = [];
  const requestBody = s.body(capture.request.body, capture.request.headers);
  if (requestBody.opaque) opaque.push('the request body');
  const request = { ...capture.request, url: s.url(capture.request.url), headers: s.headersOf(requestBody.headers), body: requestBody.bytes };
  const real = capture.response && s.body(capture.response.body, capture.response.headers ?? {});
  const response = capture.response && { ...capture.response, headers: s.headersOf(real.headers), body: real.opaque ? new Uint8Array() : real.bytes };
  const io = capture.io.map((call, i) => {
    if (call.kind === 'fetch' || call.kind === 'http') {
      const url = s.url(call.url);
      const sent = s.body(call.sent, { 'content-type': 'text/plain' });
      const answer = s.body(call.body, call.headers);
      if (sent.opaque) opaque.push(`what call ${i + 1} sent`);
      if (answer.opaque) opaque.push(`the answer to call ${i + 1}`);
      return { ...call, url, sent: sent.bytes, headers: s.headersOf(answer.headers), body: call.body == null ? null : answer.bytes, error: call.error && s.text(call.error), key: callKey(call.method, url, sent.bytes) };
    }
    if (call.kind === 'pg') {
      const text = s.text(call.text);
      const values = s.value(call.values);
      const result = call.result && (Array.isArray(call.result) ? call.result.map((r) => ({ ...r, rows: s.value(r.rows) })) : { ...call.result, rows: s.value(call.result.rows) });
      return { ...call, text, values, result, error: call.error && s.value(call.error), key: pgKey(text, values) };
    }
    if (call.kind === 'redis') {
      const args = s.value(call.args);
      return { ...call, args, reply: s.value(call.reply), error: call.error && s.value(call.error), key: redisKey(call.command, args) };
    }
    opaque.push(`call ${i + 1} (${call.kind})`);
    return call;
  });
  return { capture: { ...capture, request, response, io, effects: io.filter((call) => call.write).map((call) => call.key) }, opaque };
}
