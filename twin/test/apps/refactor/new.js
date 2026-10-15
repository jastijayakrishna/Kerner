import { send } from '../http.js';


export function createApp() {
  return (req, res) => {
    const n = Number(new URL(req.url, 'http://app').searchParams.get('n') ?? 10);
    send(res, 200, { n, total: Array.from({ length: n }, (_, i) => i + 1).reduce((a, b) => a + b, 0) });
  };
}
