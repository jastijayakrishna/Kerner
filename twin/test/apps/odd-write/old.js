import http from 'node:http';
import { send } from '../http.js';





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
