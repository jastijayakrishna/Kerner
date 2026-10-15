import { readJson, send } from '../http.js';


export function createApp() {
  return async (req, res) => {
    const order = await readJson(req);
    if (order.card) throw new Error(`cannot charge ${order.card}`);
    send(res, 200, { charged: true, receipt: order.email });
  };
}
