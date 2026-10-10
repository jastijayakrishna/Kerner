import { send } from '../http.js';

// Reads the body the way express.json() and body-parser do, with data and end events, and then charges the order.
export function createApp() {
  return (req, res) => {
    const parts = [];
    req.on('data', (chunk) => parts.push(chunk));
    req.on('end', async () => {
      const order = JSON.parse(Buffer.concat(parts).toString('utf8'));
      const paid = await fetch(`${process.env.KERN_TEST_PAY_URL}/charge`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ amount: order.amount }) });
      send(res, 200, { paid: paid.status === 200 });
    });
  };
}
