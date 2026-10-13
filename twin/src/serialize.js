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

const container = (value) => value !== null && typeof value === 'object' && !(value instanceof Uint8Array) && !(value instanceof Date);

/**
 * A value that crossed between threads, where every Buffer became a Uint8Array, with its Buffers back: in place, at any depth (a list of
 * what is left, not recursion), each array or object once.
 */
export function revive(value) {
  if (value instanceof Uint8Array) return Buffer.from(value.buffer, value.byteOffset, value.byteLength);
  if (!container(value)) return value;
  const seen = new Set([value]);
  const left = [value];
  while (left.length) {
    const holder = left.pop();
    for (const key of Array.isArray(holder) ? holder.keys() : Object.keys(holder)) {
      const item = holder[key];
      if (item instanceof Uint8Array) holder[key] = Buffer.from(item.buffer, item.byteOffset, item.byteLength);
      else if (container(item) && !seen.has(item)) {
        seen.add(item);
        left.push(item);
      }
    }
  }
  return value;
}

/**
 * The JSON text of a value, exactly as JSON.stringify writes it, at any depth: JSON.stringify gives up at a few thousand levels, so arrays
 * and objects are written with a stack of their own. Plain values still go through JSON.stringify, so strings are escaped as it escapes them.
 */
export function jsonText(value) {
  const parts = [];
  const frames = [];
  const resolved = (item, key) => (item !== null && typeof item === 'object' && typeof item.toJSON === 'function' ? item.toJSON(key) : item);
  const absent = (item) => item === undefined || typeof item === 'function' || typeof item === 'symbol';
  const open = (item) => {
    if (Array.isArray(item)) {
      parts.push('[');
      frames.push({ item, names: null, next: 0, written: 0 });
    } else if (item !== null && typeof item === 'object') {
      parts.push('{');
      frames.push({ item, names: Object.keys(item), next: 0, written: 0 });
    } else parts.push(JSON.stringify(item));
  };
  const top = resolved(value, '');
  if (absent(top)) return undefined;
  open(top);
  while (frames.length) {
    const frame = frames.at(-1);
    const size = frame.names ? frame.names.length : frame.item.length;
    if (frame.next >= size) {
      parts.push(frame.names ? '}' : ']');
      frames.pop();
      continue;
    }
    const i = frame.next++;
    if (!frame.names) {
      // In an array, what JSON cannot write is written as null.
      if (i > 0) parts.push(',');
      const item = resolved(frame.item[i], String(i));
      if (absent(item)) parts.push('null');
      else open(item);
      continue;
    }
    // In an object, a member JSON cannot write is left out.
    const name = frame.names[i];
    const item = resolved(frame.item[name], name);
    if (absent(item)) continue;
    if (frame.written++ > 0) parts.push(',');
    parts.push(`${JSON.stringify(name)}:`);
    open(item);
  }
  return parts.join('');
}
