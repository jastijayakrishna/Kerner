import { readJson, send } from '../http.js';


export function createApp() {
  return async (req, res) => {
    const order = await readJson(req);
    send(res, 200, { charged: true, receipt: order.email });
  };
}
