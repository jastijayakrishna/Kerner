import http from 'node:http';
import { createTwin } from '/twin/src/index.js';

// The shop in production: the twin serves through its own listener and records Postgres. /__twin shows its numbers to the traffic driver.
const twin = await createTwin({
  old: { module: '/shop/old/app.js', export: 'createApp' },
  candidate: { module: '/shop/new/app.js', export: 'createApp' },
  adapters: ['pg'],
  sampleRate: 1,
  queueLimit: 1000,
  epsilon: 0.05,
  minHours: 0,
});
const listener = await twin.listener();
http.createServer((req, res) => {
  if (req.url === '/__twin') {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ stats: twin.stats(), decision: twin.decision(), differences: twin.differences().slice(0, 5) }));
  }
  return listener(req, res);
}).listen(3000, () => console.log('the shop listens on :3000'));
