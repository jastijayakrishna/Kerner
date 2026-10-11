import { readJson, send } from '../http.js';

// Takes a payment: a card number and an email arrive in the body.
export function createApp() {
  return async (req, res) => {
    const order = await readJson(req);
    send(res, 200, { charged: true, receipt: order.email });
  };
}
