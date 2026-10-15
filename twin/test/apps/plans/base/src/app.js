
const PLANS = [
  {
    name: 'free',
    max: 1,
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
