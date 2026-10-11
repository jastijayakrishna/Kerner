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

/** A value as one short line, for showing a difference. */
export function preview(value) {
  const text = value === undefined ? 'undefined' : (JSON.stringify(value) ?? String(value));
  return text.length > 120 ? `${text.slice(0, 117)}...` : text;
}

const kindOf = (value) => (value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value);

/** Where two values differ, as paths such as $.items[2].price; at most `limit` of them. */
export function diffValues(a, b, path = '$', out = [], limit = 20) {
  if (out.length >= limit || Object.is(a, b)) return out;
  const kind = kindOf(a);
  if (kind !== kindOf(b) || (kind !== 'object' && kind !== 'array')) {
    out.push({ path, old: preview(a), new: preview(b) });
    return out;
  }
  if (kind === 'array') {
    if (a.length !== b.length) out.push({ path: `${path}.length`, old: String(a.length), new: String(b.length) });
    for (let i = 0; i < Math.min(a.length, b.length) && out.length < limit; i++) diffValues(a[i], b[i], `${path}[${i}]`, out, limit);
    return out;
  }
  const names = [...new Set([...Object.keys(a), ...Object.keys(b)])].sort();
  for (const name of names) {
    if (out.length >= limit) break;
    const child = /^[A-Za-z_$][\w$]*$/.test(name) ? `${path}.${name}` : `${path}[${JSON.stringify(name)}]`;
    if (!Object.hasOwn(b, name)) out.push({ path: child, old: preview(a[name]), new: 'missing' });
    else if (!Object.hasOwn(a, name)) out.push({ path: child, old: 'missing', new: preview(b[name]) });
    else diffValues(a[name], b[name], child, out, limit);
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
