import { send } from '../http.js';

// Keeps the rate in memory once it has fetched it: the real app's cache warms, while a fresh ghost's starts cold.
let cached = null;

export function createApp() {
  return async (req, res) => {
    cached ??= await (await fetch(`${process.env.KERN_TEST_RATES}/eur`)).json();
    send(res, 200, { rate: cached.rate });
  };
}
