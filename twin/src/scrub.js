import { createHmac, randomBytes } from 'node:crypto';
import { callKey, pgKey, redisKey } from './keys.js';
import { decodeBody, exactBytes, jsonText } from './serialize.js';

// Before a case leaves the twin, private values are replaced by stand-ins.
// - The same value always gets the same stand-in within one case: an HMAC under a key that is thrown away. Nothing links one case's
//   stand-ins to another's.
// - A stand-in has the value's shape: an email for an email, a valid test card number for a card number, a valid UUID for a UUID,
//   letters for letters and digits for digits (a hex value staying hex). So the app's own checks and comparisons still hold for them.
// - Every search takes time in proportion to the text, so a large body cannot stall the twin's thread.

export const DEFAULT_SCRUB = {
  headers: ['authorization', 'proxy-authorization', 'cookie', 'set-cookie', 'x-api-key', 'x-auth-token', 'x-csrf-token'],
  keys: ['password', 'passwd', 'secret', 'token', 'apikey', 'api_key', 'accesstoken', 'access_token', 'refreshtoken', 'refresh_token', 'ssn',
    'cardnumber', 'card_number', 'card', 'cvv', 'cvc', 'iban'],
};

// An email's two parts have at most 64 and 253 characters (RFC 5321). Bounding them keeps the search linear on long text without an @.
const EMAIL = /[A-Za-z0-9._%+-]{1,64}@[A-Za-z0-9.-]{1,253}\.[A-Za-z]{2,63}/g;
const CARD = /\b\d(?:[ -]?\d){12,18}\b/g;
const BEARER = /\b(Bearer|Basic|Token)\s+([A-Za-z0-9._~+/=-]{3,})/gi;
const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi;
const IS_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
// A run of 32 or more token characters. Whether it holds a digit and a letter is checked after: lookaheads in the search would read the
// run again from every '-' inside it.
const RUN = /\b[A-Za-z0-9_-]{32,}\b/g;
// A text body that is nothing but name=value pairs, read as a form.
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

/** A scrubber for one case: stand-ins consistent within it, keyed by a secret no one keeps. `rules` adds `headers` and `keys`. */
export function scrubber(rules = {}) {
  const secret = randomBytes(32);
  const headers = new Set([...DEFAULT_SCRUB.headers, ...(rules.headers ?? [])].map((name) => name.toLowerCase()));
  const keys = new Set([...DEFAULT_SCRUB.keys, ...(rules.keys ?? [])].map((name) => name.toLowerCase()));
  const digest = (value) => createHmac('sha256', secret).update(String(value)).digest();
  /** The same length; letters for letters, digits for digits, a hex value's letters staying from a to f; anything else kept. */
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
  /** A UUID for a UUID, the same in either case. Its version (place 14) and variant (place 19) are kept, so a strict check still passes. */
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
  /**
   * One value: a value a pattern knows (a card number, an email) gets that pattern's stand-in even under a private name, so an app's
   * own checks (a card's checksum, say) still pass; any other value under a private name is shaped.
   */
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
  /**
   * A value rebuilt with its private values replaced, at any depth: arrays and objects are walked with a stack of their own, never by
   * recursion, so a body nested a hundred thousand deep is scrubbed in full. An array's items are judged by the array's own name (a list
   * of tokens is a list of private values), an object's members by their names. A circle, which JSON cannot hold, is cut.
   */
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
  // The content type is kept as it is: it holds nothing private, and a file upload's parts are found by the boundary it names.
  const headersOf = (list) => Object.fromEntries(Object.entries(list ?? {}).map(([name, v]) => {
    const lower = name.toLowerCase();
    const one = (item) => (headers.has(lower) ? '[scrubbed]' : lower === 'content-type' ? item : text(item));
    return [name, Array.isArray(v) ? v.map(one) : one(v)];
  }));
  /**
   * One piece of an address or a form (a path segment, a name or a value), decoded to be scrubbed and written back as it was written:
   * the characters it left unencoded stay unencoded, and in a query '+' stays a space. A piece that did not change is kept exactly.
   */
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
  /** A query string or a form body, pair by pair: order and repeated names kept, a value under a private name always replaced. */
  const query = (raw) => raw.split('&').map((pair) => {
    const at = pair.indexOf('=');
    if (at < 0) return piece(pair, text, true);
    const name = pair.slice(0, at);
    let key = name;
    try {
      key = decodeURIComponent(name.replace(/\+/g, ' '));
    } catch {
      // A name that does not decode is looked up as written.
    }
    const sensitive = keys.has(key.toLowerCase());
    return `${piece(name, text, true)}=${piece(pair.slice(at + 1), (v) => scalar(v, sensitive), true)}`;
  }).join('&');
  /**
   * An address with its private values replaced, piece by piece. Its scheme and host, and every piece that did not change, are kept as
   * written, so the ghosts still find a recorded call by its address.
   */
  const url = (raw) => {
    const [, origin = '', path, search, hash = ''] = /^([A-Za-z][A-Za-z0-9+.-]*:\/\/[^/?#]*)?([^?#]*)(?:\?([^#]*))?(#[\s\S]*)?$/.exec(String(raw));
    const pieces = path.split('/').map((segment) => piece(segment, text, false)).join('/');
    return `${origin}${pieces}${search === undefined ? '' : `?${query(search)}`}${hash && text(hash)}`;
  };
  /**
   * A body with its private values replaced, if it can be read: JSON by its values, a form pair by pair, other text by its patterns,
   * unpacked first if it was compressed. Answers { bytes, headers, opaque }; `opaque` means the body could not be read and is kept as
   * it was.
   */
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
        // Not JSON after all: read as text below.
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

/**
 * The capture of one request with its private values replaced, everywhere one can be: the address, the headers and the body of the
 * request, every recorded call (its address, what it sent, what it answered), and the real answer. Each call is named again from its
 * scrubbed payload, so the ghosts still find it. Answers { capture, opaque }: `opaque` names the places that could not be read.
 */
export function scrubCapture(capture, rules) {
  const s = scrubber(rules);
  const opaque = [];
  const requestBody = s.body(capture.request.body, capture.request.headers);
  if (requestBody.opaque) opaque.push('the request body');
  const request = { ...capture.request, url: s.url(capture.request.url), headers: s.headersOf(requestBody.headers), body: requestBody.bytes };
  // The real answer never goes into a case, only the ghosts' answers on the scrubbed copy do. It is scrubbed all the same, and emptied
  // when it cannot be read, so nothing in a scrubbed capture keeps a real value.
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
