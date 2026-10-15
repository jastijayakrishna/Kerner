import { performance } from 'node:perf_hooks';
import http from 'node:http';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../src/index.js';

const root = (version) => fileURLToPath(new URL(`../test/apps/three-prs/${version}/`, import.meta.url));
const side = (version) => ({ module: join(root(version), 'src', 'app.js'), export: 'createApp', root: root(version) });
const manifest = fileURLToPath(new URL('../test/apps/three-prs/manifest.json', import.meta.url));
const body = JSON.stringify({ items: [{ price: 2, count: 3 }] });

async function rate(label, options, total = 600) {
  const twin = await createTwin({ old: side('base'), candidate: side('head'), queueLimit: 10000, ...options });
  const server = http.createServer(await twin.listener());
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const send = async () => (await fetch(`http://127.0.0.1:${server.address().port}/total`, { method: 'POST', headers: { 'content-type': 'application/json' }, body })).arrayBuffer();
  for (let i = 0; i < 50; i++) await send();
  await twin.idle();
  const before = twin.stats().same;
  const started = performance.now();
  for (let i = 0; i < total; i++) await send();
  await twin.idle();
  const seconds = (performance.now() - started) / 1000;
  const judged = twin.stats().same - before;
  console.log(`${label.padEnd(26)} ${(judged / seconds).toFixed(0).padStart(6)} requests judged per second (${judged} same, ${twin.stats().different} different)`);
  server.closeAllConnections();
  server.close();
  await twin.close();
}

await rate('without measuring lines', {});
await rate('measuring changed lines', { manifest });
