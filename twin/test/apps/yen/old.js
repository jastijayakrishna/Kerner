import { readJson, send } from '../http.js';

export function createApp() {
  return async (req, res) => {
    const { amount, currency } = await readJson(req);
    const total = currency === 'JPY' ? Math.round(amount * 0.9) : Math.round(amount * 90) / 100;
    send(res, 200, { currency, total });
  };
}
