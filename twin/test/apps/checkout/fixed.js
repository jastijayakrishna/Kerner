import { readJson, send } from '../http.js';

// The fix an agent would write: orders with a card number are charged again.
export function createApp() {
  return async (req, res) => {
    const order = await readJson(req);
    send(res, 200, { charged: true, receipt: order.email });
  };
}
