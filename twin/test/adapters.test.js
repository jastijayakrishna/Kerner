import { test } from 'node:test';
import assert from 'node:assert/strict';
import { listen, production } from './helpers.js';

async function upstream(counts) {
  return listen((req, res) => {
    counts[req.url] = (counts[req.url] ?? 0) + 1;
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(req.url === '/rate' ? '{"rate":2}' : '{"ok":true}');
    });
  });
}

test('node:http client calls are recorded in the app and replayed to the ghosts, so nothing is charged twice', async () => {
  const counts = {};
  const server = await upstream(counts);
  process.env.KERN_TEST_UPSTREAM = server.url;
  try {
    const shop = await production('http-client', { adapters: ['http'] });
    assert.deepEqual(shop.twin.stats().adapters, { fetch: 'recording', http: 'recording' });
    assert.deepEqual(await (await fetch(`${shop.url}/rate`)).json(), { rate: 2 });
    assert.deepEqual(await (await fetch(`${shop.url}/order`)).json(), { charged: 200 });
    await shop.twin.idle();
    assert.deepEqual(counts, { '/rate': 1, '/charge': 1 }, 'only the real app reached the upstream');
    const stats = shop.twin.stats();
    assert.deepEqual([stats.same, stats.different, stats.notCompared], [1, 1, 0], JSON.stringify(stats));
    assert.ok(shop.twin.differences()[0].kinds.includes('effects'), JSON.stringify(shop.twin.differences()));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_UPSTREAM;
    await server.close();
  }
});

test('without the http adapter, a node:http call leaves the request not compared, and nothing real happens', async () => {
  const counts = {};
  const server = await upstream(counts);
  process.env.KERN_TEST_UPSTREAM = server.url;
  try {
    const shop = await production('http-client');
    assert.deepEqual(await (await fetch(`${shop.url}/rate`)).json(), { rate: 2 });
    await shop.twin.idle();
    assert.deepEqual(counts, { '/rate': 1 });
    assert.equal(shop.twin.stats().notCompared, 1);
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_UPSTREAM;
    await server.close();
  }
});
