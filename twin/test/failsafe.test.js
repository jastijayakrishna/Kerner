import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tmpdir } from 'node:os';
import { wrapListener } from '../src/capture.js';
import { app, production, post, listen } from './helpers.js';

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

test('a ghost that ended its own process after answering is named on the request that left that work running, and the next one is judged on a fresh pair', async () => {
  for (const ghosts of ['thread', 'process']) {
    const shop = await production('quits', { ghosts, timeoutMs: 8000 });
    assert.equal((await fetch(`${shop.url}/?n=2`)).status, 200);
    await shop.twin.idle();
    // The new version's slow background work ends, and its ghost with it: between two requests, after its answer was sent.
    await new Promise((resolve) => setTimeout(resolve, 1000));
    const asked = Date.now();
    assert.equal((await fetch(`${shop.url}/?n=3`)).status, 200);
    await shop.twin.idle();
    const took = Date.now() - asked;
    const stats = shop.twin.stats();
    assert.equal(stats.same, 2, `${ghosts}: ${JSON.stringify(stats)}`);
    const [found] = shop.twin.differences();
    assert.equal(found?.url, '/?n=2', `${ghosts}: ${JSON.stringify(found)}`);
    assert.deepEqual(found.details[0], { kind: 'crash', path: 'after the answer', old: 'none', new: 'the ghost stopped (exit 0)' }, ghosts);
    assert.ok(took < 4000, `${ghosts}: judged ${took} ms after it was asked, with a time limit of 8000 ms`);
    assert.equal(stats.disabled, null);
    await shop.close();
  }
});

test('a ghost that ran out of memory after answering is named on the request that left that work running', async () => {
  const shop = await production('late-hog', { ghostMemoryMb: 64 });
  assert.equal((await fetch(`${shop.url}/?n=2`)).status, 200);
  await shop.twin.idle();
  // The new version's background work fills its ghost's memory between two requests: the ghost reports a failure, and stops.
  await new Promise((resolve) => setTimeout(resolve, 1500));
  assert.equal((await fetch(`${shop.url}/?n=3`)).status, 200);
  await shop.twin.idle();
  const stats = shop.twin.stats();
  const [found] = shop.twin.differences();
  assert.equal(found?.url, '/?n=2', JSON.stringify({ stats, found }));
  assert.equal(found.details[0].path, 'after the answer');
  assert.match(found.details[0].new, /the ghost (failed|stopped)/);
  assert.equal(stats.same, 2, JSON.stringify(stats));
  assert.equal(stats.disabled, null);
  await shop.close();
});

test('a new version that answers more than maxBodyBytes is compared by size and sha256, and the twin stays on', async () => {
  const shop = await production('big-answer', { maxBodyBytes: 1 << 20 });
  assert.deepEqual(await (await fetch(shop.url)).json(), { count: 70000 });
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.equal(stats.disabled, null, stats.disabled);
  const [found] = shop.twin.differences();
  assert.deepEqual(found?.kinds, ['body'], JSON.stringify(stats));
  assert.equal(found.details[0].path, 'body', JSON.stringify(found.details[0]));
  const size = Number(/^(\d+) bytes, sha256 [0-9a-f]{16}$/.exec(found.details[0].new)?.[1]);
  assert.ok(size > 2 * (1 << 20), found.details[0].new);
  await shop.close();
  // Both ghosts answer the same table, each over the cap: the same.
  const both = await production('big-answer', { maxBodyBytes: 1 << 20, old: app('big-answer', 'new') });
  await (await fetch(both.url)).arrayBuffer();
  await both.twin.idle();
  assert.equal(both.twin.stats().same, 1, JSON.stringify(both.twin.stats()));
  await both.close();
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

test('an answer nested fifty thousand levels deep is judged like any other, and the twin stays on', async () => {
  const shop = await production('echo');
  const deep = `${'['.repeat(50000)}1${']'.repeat(50000)}`;
  const answer = await fetch(shop.url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: deep });
  assert.equal(await answer.text(), deep);
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.equal(stats.disabled, null, stats.disabled);
  assert.equal(stats.same, 1, JSON.stringify(stats));
  await shop.close();
});
