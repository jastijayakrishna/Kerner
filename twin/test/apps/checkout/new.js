import { readJson, send } from '../http.js';

// Breaks on every order that carries a card number, and says the number in its error.
export function createApp() {
  return async (req, res) => {
    const order = await readJson(req);
    if (order.card) throw new Error(`cannot charge ${order.card}`);
    send(res, 200, { charged: true, receipt: order.email });
  };
}
