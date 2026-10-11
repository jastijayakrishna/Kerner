import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { app, production } from './helpers.js';

test('a running twin never keeps the app\'s process alive', () => {
  // An app that closes its server expects its process to end; the twin's thread and its ghosts must not hold it open.
  const file = join(tmpdir(), `kern-twin-exit-${process.pid}.mjs`);
  writeFileSync(file, `
import http from 'node:http';
import { createTwin } from ${JSON.stringify(new URL('../src/index.js', import.meta.url).href)};
import { createApp } from ${JSON.stringify(new URL('./apps/refactor/old.js', import.meta.url).href)};
const twin = await createTwin(${JSON.stringify({ old: app('refactor', 'old'), candidate: app('refactor', 'new') })});
const server = http.createServer(twin.wrap(createApp()));
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
await (await fetch('http://127.0.0.1:' + server.address().port + '/?n=2')).arrayBuffer();
await twin.idle();
server.closeAllConnections();
server.close();
console.log(JSON.stringify(twin.stats()));
`);
  try {
    const out = execFileSync(process.execPath, [file], { encoding: 'utf8', timeout: 20000 });
    assert.equal(JSON.parse(out).same, 1, out);
  } finally {
    rmSync(file, { force: true });
  }
});

test('counts, differences, the decision and the evidence all come from the twin\'s own thread', async () => {
  const shop = await production('renamed-field', { epsilon: 0.5, minHours: 0 });
  await (await fetch(`${shop.url}/user`)).arrayBuffer();
  await shop.twin.idle();
  assert.equal(shop.twin.stats().different, 1);
  const [found] = shop.twin.differences();
  assert.deepEqual([found.route, found.segment, found.serving, found.outcome], ['GET /user', 'all', 'old', 'different']);
  assert.deepEqual(shop.twin.decision().reasons, ['1 unexplained difference', 'all traffic: 0 of 6 clean comparisons']);
  const evidence = shop.twin.evidence();
  assert.match(evidence.candidate, /^sha256:[0-9a-f]{64}$/);
  assert.equal(evidence.totals['*'].different, 1);
  assert.equal(evidence.unexplained[0].route, 'GET /user');
  await shop.close();
});

test('when the twin\'s thread fails, customers are served as always and the twin says why', async () => {
  process.env.KERN_TWIN_FAULT = 'thread-exit';
  try {
    const shop = await production('refactor');
    assert.equal((await fetch(`${shop.url}/?n=2`)).status, 200);
    await shop.twin.idle();
    assert.match(shop.twin.stats().disabled, /the twin's thread stopped \(exit 9\)/);
    assert.deepEqual(await (await fetch(`${shop.url}/?n=3`)).json(), { n: 3, total: 6 });
    await shop.close();
  } finally {
    delete process.env.KERN_TWIN_FAULT;
  }
});
