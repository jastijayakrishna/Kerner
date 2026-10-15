import { send } from '../http.js';


let cached = null;

export function createApp() {
  return async (req, res) => {
    cached ??= await (await fetch(`${process.env.KERN_TEST_RATES}/eur`)).json();
    send(res, 200, { rate: cached.rate });
  };
}
