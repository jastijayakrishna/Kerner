import { gunzipSync, inflateSync, brotliDecompressSync } from 'node:zlib';
import { digest } from './keys.js';

const MAX_DECODED = 16 << 20;

/** The body as the client reads it: compressed bodies are unpacked, up to 16 MiB. */
export function decodeBody(body, headers) {
  const encoding = String(headers['content-encoding'] ?? '').toLowerCase().trim();
  const options = { maxOutputLength: MAX_DECODED };
  if (encoding === 'gzip' || encoding === 'x-gzip') return gunzipSync(body, options);
  if (encoding === 'deflate') return inflateSync(body, options);
  if (encoding === 'br') return brotliDecompressSync(body, options);
  return body;
}

/** JSON and text bodies are compared by value; anything else by a hash of its bytes. */
export function readBody(body, headers) {
  let bytes;
  try {
    bytes = decodeBody(body, headers);
  } catch {
    return { kind: 'bytes', digest: digest(body) };
  }
  const type = String(headers['content-type'] ?? '').toLowerCase();
  if (type.includes('json')) {
    try {
      return { kind: 'json', value: JSON.parse(bytes.toString('utf8')) };
    } catch {
      // Not valid JSON after all: compared as text below.
    }
  }
  if (type.includes('json') || type.startsWith('text/') || type.includes('xml') || type.includes('javascript')) return { kind: 'text', value: bytes.toString('utf8') };
  return { kind: 'bytes', digest: digest(bytes) };
}

/** A value as one short line, for showing a difference. A value nested too deeply for JSON.stringify is shown by its kind. */
export function preview(value) {
  let text;
  try {
    text = value === undefined ? 'undefined' : (JSON.stringify(value) ?? String(value));
  } catch {
    text = Array.isArray(value) ? `an array of ${value.length}, nested too deeply to show` : `an object of ${Object.keys(value).length} keys, nested too deeply to show`;
  }
  return text.length > 120 ? `${text.slice(0, 117)}...` : text;
}

const kindOf = (value) => (value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value);
// A path as shown: one that runs deeper than this many characters keeps its start and its end.
const MOST_PATH = 200;
const shown = (path) => (path.length > MOST_PATH ? `${path.slice(0, 100)}...${path.slice(-97)}` : path);

/**
 * Where two values differ, as paths such as $.items[2].price, in the order of the values; at most `limit` of them. The values are walked
 * with a stack of their own, one frame per array or object being compared, not by recursion: JSON.parse reads a body nested a hundred
 * thousand deep, and comparing it must not run out of stack. A frame hands out one child at a time, so a wide array costs no more memory
 * than a narrow one.
 */
export function diffValues(a, b, path = '$', out = [], limit = 20) {
  const frames = [];
  const visit = (x, y, at) => {
    if (Object.is(x, y)) return;
    const kind = kindOf(x);
    if (kind !== kindOf(y) || (kind !== 'object' && kind !== 'array')) {
      out.push({ path: shown(at), old: preview(x), new: preview(y) });
    } else if (kind === 'array') {
      if (x.length !== y.length) out.push({ path: shown(`${at}.length`), old: String(x.length), new: String(y.length) });
      frames.push({ x, y, at, names: null, next: 0, size: Math.min(x.length, y.length) });
    } else {
      const names = [...new Set([...Object.keys(x), ...Object.keys(y)])].sort();
      frames.push({ x, y, at, names, next: 0, size: names.length });
    }
  };
  visit(a, b, path);
  while (frames.length && out.length < limit) {
    const frame = frames.at(-1);
    if (frame.next >= frame.size) {
      frames.pop();
      continue;
    }
    const i = frame.next++;
    const { x, y, at, names } = frame;
    if (!names) {
      visit(x[i], y[i], `${at}[${i}]`);
      continue;
    }
    const name = names[i];
    const child = /^[A-Za-z_$][\w$]*$/.test(name) ? `${at}.${name}` : `${at}[${JSON.stringify(name)}]`;
    if (!Object.hasOwn(y, name)) out.push({ path: shown(child), old: preview(x[name]), new: 'missing' });
    else if (!Object.hasOwn(x, name)) out.push({ path: shown(child), old: 'missing', new: preview(y[name]) });
    else visit(x[name], y[name], child);
  }
  return out;
}

/**
 * The parts as one Uint8Array of exactly their size. A Buffer may be a slice of Node's shared 8 KiB pool, and posting it to another
 * thread would carry the whole pool along, other requests' data included (measured); this copy carries only its own bytes.
 */
export function exactBytes(parts) {
  let size = 0;
  for (const part of parts) size += part.byteLength;
  const out = new Uint8Array(size);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.byteLength;
  }
  return out;
}

/** A value that crossed between threads, where every Buffer became a Uint8Array, with its Buffers back (in place, deeply). */
export function revive(value) {
  if (value instanceof Uint8Array) return Buffer.from(value.buffer, value.byteOffset, value.byteLength);
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) value[i] = revive(value[i]);
    return value;
  }
  if (value && typeof value === 'object' && !(value instanceof Date)) {
    for (const key of Object.keys(value)) value[key] = revive(value[key]);
  }
  return value;
}
