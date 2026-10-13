import http from 'node:http';
import { send } from '../http.js';

/**
 * Writes to an outgoing request what Node itself refuses (a number) and what it takes without a word (text in an encoding it does not
 * know, which it never checks before the request is sent), then drops the request. Answers what each write did.
 */
export function tryWrites() {
  const out = http.request(new URL('/charge', process.env.KERN_TEST_UPSTREAM), { method: 'POST' });
  out.on('error', () => {});
  const seen = [];
  for (const [chunk, encoding] of [[5, undefined], ['x', 'nope']]) {
    try {
      out.write(chunk, encoding);
      seen.push('written');
    } catch (error) {
      seen.push(`${error.code}: ${error.message}`);
    }
  }
  out.destroy();
  return seen;
}

export function createApp() {
  return (req, res) => send(res, 200, { seen: tryWrites() });
}
