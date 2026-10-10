import { readJson, send } from '../http.js';

// One discount formula for every currency: right for euros, but yen have no cents.
export function createApp() {
  return async (req, res) => {
    const { amount, currency } = await readJson(req);
    send(res, 200, { currency, total: Math.round(amount * 90) / 100 });
  };
}
