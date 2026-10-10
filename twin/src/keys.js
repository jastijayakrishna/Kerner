import { createHash } from 'node:crypto';

/** Methods that only read; every other method changes something somewhere. */
export const READS = new Set(['GET', 'HEAD', 'OPTIONS']);

export function digest(data) {
  return createHash('sha256').update(data).digest('hex');
}

/** One outgoing call, named the same way in the real run and in both ghosts: method, address, and a short hash of the body. */
export function callKey(method, url, body) {
  const verb = String(method).toUpperCase();
  return `${verb} ${url} ${body && body.length ? digest(body).slice(0, 16) : '-'}`;
}

export function isWrite(method) {
  return !READS.has(String(method).toUpperCase());
}
