import { send } from '../http.js';

// The same body reading, with the retry that charges twice whenever the payment service answers 200 instead of 201.
const charge = (amount) => fetch(`${process.env.KERN_TEST_PAY_URL}/charge`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ amount }) });

export function createApp() {
  return (req, res) => {
    const parts = [];
    req.on('data', (chunk) => parts.push(chunk));
    req.on('end', async () => {
      const order = JSON.parse(Buffer.concat(parts).toString('utf8'));
      let paid = await charge(order.amount);
      if (paid.status !== 201) paid = await charge(order.amount);
      send(res, 200, { paid: paid.status === 200 });
    });
  };
}
