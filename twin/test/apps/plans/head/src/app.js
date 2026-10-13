// A shop's plans, written the way Prettier writes an array of objects: each opening brace alone on its line.
const PLANS = [
  {
    name: 'free',
    max: 1,
  },
  {
    name: 'team',
    max: 10,
  },
];

export function createApp() {
  return (req, res) => {
    const name = new URL(req.url, 'http://shop').searchParams.get('plan') ?? 'free';
    const plan = PLANS.find((p) => p.name === name);
    res.writeHead(plan ? 200 : 404, { 'content-type': 'application/json' });
    res.end(JSON.stringify(plan ?? { error: 'no such plan' }));
  };
}
