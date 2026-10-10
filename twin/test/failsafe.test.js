import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tmpdir } from 'node:os';
import { wrapListener } from '../src/capture.js';
import { production, post, listen } from './helpers.js';

test('if the twin\'s own recording fails, the request is served as always and the twin is told', async () => {
  const problems = [];
  const listener = wrapListener((req, res) => res.end('ok'), {
    sample: () => { throw new Error('boom'); },
    maxBodyBytes: 1024,
    deliver: () => {},
    onError: (error) => problems.push(error.message),
  });
  const server = await listen(listener);
  assert.equal(await (await fetch(server.url)).text(), 'ok');
  assert.deepEqual(problems, ['boom']);
  await server.close();
});

test('an outgoing call whose answer is too large to copy leaves the request skipped, and the customer served', async () => {
  const pay = await listen((req, res) => {
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ok: true, note: 'x'.repeat(4096) }));
    });
  });
  process.env.KERN_TEST_PAY_URL = pay.url;
  try {
    const shop = await production('double-charge', { maxBodyBytes: 1024 });
    assert.deepEqual(await (await post(shop.url, { amount: 25 })).json(), { paid: true });
    await shop.twin.idle();
    assert.equal(shop.twin.stats().skipped, 1);
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_PAY_URL;
    await pay.close();
  }
});

test('a new version that runs out of memory only ends its ghost', async () => {
  const shop = await production('memory-hog', { ghostMemoryMb: 64 });
  assert.equal((await fetch(`${shop.url}/?n=2`)).status, 200);
  await shop.twin.idle();
  assert.ok(shop.twin.differences()[0].kinds.includes('crash'), JSON.stringify(shop.twin.differences()));
  assert.equal(shop.twin.stats().disabled, null);
  await shop.close();
});

test('a ghost pair that keeps failing switches the twin off instead of restarting forever', async () => {
  const shop = await production('hang', { timeoutMs: 300, maxRestarts: 1 });
  for (const n of [1, 2]) {
    assert.equal((await fetch(`${shop.url}/?n=${n}`)).status, 200);
    await shop.twin.idle();
  }
  assert.match(shop.twin.stats().disabled, /restart more than 1 times/);
  assert.equal((await fetch(`${shop.url}/?n=3`)).status, 200);
  await shop.close();
});

test('a log file that cannot be written loses no difference', async () => {
  const shop = await production('renamed-field', { log: tmpdir() });
  await (await fetch(`${shop.url}/user`)).arrayBuffer();
  await shop.twin.idle();
  assert.equal(shop.twin.stats().logErrors, 1);
  assert.equal(shop.twin.differences().length, 1);
  await shop.close();
});

test('a new version that does not even load switches the twin off, and customers are served as always', async () => {
  const shop = await production('broken');
  assert.match(shop.twin.stats().disabled, /could not start[\s\S]*does not load/);
  assert.deepEqual(await (await fetch(`${shop.url}/?n=3`)).json(), { n: 3, total: 6 });
  await shop.close();
});

test('a new version that never answers is stopped, and the twin carries on', async () => {
  const shop = await production('hang', { timeoutMs: 500 });
  assert.deepEqual(await (await fetch(`${shop.url}/?n=3`)).json(), { n: 3, total: 6 });
  await shop.twin.idle();
  assert.ok(shop.twin.differences()[0].kinds.includes('hang'), JSON.stringify(shop.twin.differences()));
  assert.equal(shop.twin.stats().restarts, 1);
  assert.equal((await fetch(`${shop.url}/?n=4`)).status, 200);
  await shop.twin.idle();
  assert.equal(shop.twin.stats().different, 2);
  await shop.close();
});

test('a new version that ends its own process only ends its ghost', async () => {
  const shop = await production('exits');
  assert.equal((await fetch(`${shop.url}/?n=2`)).status, 200);
  await shop.twin.idle();
  assert.ok(shop.twin.differences()[0].kinds.includes('crash'));
  assert.equal(shop.twin.stats().disabled, null);
  await shop.close();
});

test('a request too large to copy is served and counted as skipped', async () => {
  const shop = await production('yen', { maxBodyBytes: 1024 });
  const answer = await post(shop.url, { amount: 1, currency: 'EUR', note: 'x'.repeat(4096) });
  assert.deepEqual(await answer.json(), { currency: 'EUR', total: 0.9 });
  await shop.twin.idle();
  assert.equal(shop.twin.stats().skipped, 1);
  await shop.close();
});

test('when the ghosts fall behind, customers are served as always and the overflow is counted', async () => {
  const shop = await production('slow', { queueLimit: 1 });
  const statuses = await Promise.all(Array.from({ length: 12 }, (_, i) => fetch(`${shop.url}/?n=${i}`).then((r) => r.status)));
  assert.ok(statuses.every((s) => s === 200));
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.ok(stats.dropped > 0, JSON.stringify(stats));
  assert.equal(stats.sampled + stats.dropped, 12, JSON.stringify(stats));
  assert.equal(stats.same, stats.sampled, JSON.stringify(stats));
  await shop.close();
});

test('KERN_TWIN=off keeps the twin from starting at all', async () => {
  process.env.KERN_TWIN = 'off';
  try {
    const shop = await production('refactor');
    assert.equal(shop.twin.stats().disabled, 'switched off with KERN_TWIN=off');
    assert.equal((await fetch(`${shop.url}/?n=1`)).status, 200);
    await shop.close();
  } finally {
    delete process.env.KERN_TWIN;
  }
});
