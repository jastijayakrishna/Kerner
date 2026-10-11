import { send } from '../http.js';

// Reads its body the way body parsers do (express.json, raw-body): with 'data' and 'end' listeners. Then it calls a payment service.
export function createApp() {
  return (req, res) => {
    const parts = [];
    req.on('data', (part) => parts.push(part));
    req.on('end', async () => {
      const order = JSON.parse(Buffer.concat(parts).toString('utf8') || '{}');
      const paid = await fetch(`${process.env.KERN_TEST_PAY_URL}/charge`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ amount: order.amount }) });
      send(res, 200, { paid: paid.status === 200 });
    });
  };
}
