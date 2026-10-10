import { send } from '../http.js';

export function createApp() {
  return (req, res) => {
    const n = Number(new URL(req.url, 'http://app').searchParams.get('n') ?? 10);
    let total = 0;
    for (let i = 1; i <= n; i++) total += i;
    send(res, 200, { n, total });
  };
}
