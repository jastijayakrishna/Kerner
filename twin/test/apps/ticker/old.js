import { readJson, send } from '../http.js';



export function createApp() {
  setInterval(() => {
    fetch(`${process.env.KERN_TEST_PAY_URL}/heartbeat`, { method: 'POST' }).catch(() => {});
  }, 2).unref();
  return async (req, res) => {
    const order = await readJson(req);
    const paid = await fetch(`${process.env.KERN_TEST_PAY_URL}/charge`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ amount: order.amount }) });
    send(res, 200, { paid: paid.status === 200 });
  };
}
