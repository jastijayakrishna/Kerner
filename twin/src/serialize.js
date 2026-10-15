import { gunzipSync, inflateSync, brotliDecompressSync } from 'node:zlib';
import { digest } from './keys.js';

const MAX_DECODED = 16 << 20;

export function decodeBody(body, headers) {
  const encoding = String(headers['content-encoding'] ?? '').toLowerCase().trim();
  const options = { maxOutputLength: MAX_DECODED };
  if (encoding === 'gzip' || encoding === 'x-gzip') return gunzipSync(body, options);
  if (encoding === 'deflate') return inflateSync(body, options);
  if (encoding === 'br') return brotliDecompressSync(body, options);
  return body;
}

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
    }
  }
  if (type.includes('json') || type.startsWith('text/') || type.includes('xml') || type.includes('javascript')) return { kind: 'text', value: bytes.toString('utf8') };
  return { kind: 'bytes', digest: digest(bytes) };
}

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
const MOST_PATH = 200;
const shown = (path) => (path.length > MOST_PATH ? `${path.slice(0, 100)}...${path.slice(-97)}` : path);

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

export { jsonText } from './keys.js';
