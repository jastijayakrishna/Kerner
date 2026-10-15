import { readJson, send } from '../http.js';


const charge = (amount) => fetch(`${process.env.KERN_TEST_PAY_URL}/charge`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ amount }) });

export function createApp() {
  return async (req, res) => {
    const order = await readJson(req);
    let paid = await charge(order.amount);
    if (paid.status !== 201) paid = await charge(order.amount);
    send(res, 200, { paid: paid.status === 200 });
  };
}
