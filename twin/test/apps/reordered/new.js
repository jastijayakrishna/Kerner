import { readJson, send } from '../http.js';


export function createApp() {
  return async (req, res) => {
    const { currency, amount } = await readJson(req);
    const answer = await fetch(`${process.env.KERN_TEST_PAY}/charge`, {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ currency, amount }),
    });
    send(res, 200, { charged: answer.status === 200 });
  };
}
