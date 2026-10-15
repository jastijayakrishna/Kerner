import http from 'node:http';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../src/index.js';
import { createApp } from '../test/apps/refactor/old.js';

const app = (version) => ({ module: fileURLToPath(new URL(`../test/apps/refactor/${version}.js`, import.meta.url)), export: 'createApp' });

async function measure(label, listener, { total = 4000, lanes = 16, report = true } = {}) {
  const server = http.createServer(listener);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${server.address().port}/?n=50`;
  const times = [];
  let sent = 0;
  const lane = async () => {
    while (sent < total) {
      sent++;
      const t = performance.now();
      await (await fetch(url)).arrayBuffer();
      times.push(performance.now() - t);
    }
  };
  await Promise.all(Array.from({ length: lanes }, lane));
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  if (!report) return;
  times.sort((a, b) => a - b);
  const at = (q) => times[Math.min(times.length - 1, Math.floor(times.length * q))].toFixed(3);
  console.log(`${label.padEnd(24)} p50 ${at(0.5)} ms   p99 ${at(0.99)} ms`);
}

await measure('warm-up', createApp(), { report: false });
await measure('no twin', createApp());
for (const sampleRate of [0, 0.1, 1]) {
  const twin = await createTwin({ old: app('old'), candidate: app('new'), sampleRate, queueLimit: 10000 });
  await measure(`twin sampling ${sampleRate * 100}%`, await twin.listener());
  await twin.idle();
  const s = twin.stats();
  console.log(`  compared ${s.same + s.stated + s.different} (same ${s.same}), not compared ${s.notCompared}, skipped ${s.skipped}, dropped ${s.dropped}`);
  await twin.close();
}
