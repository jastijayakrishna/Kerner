import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { listen, production } from './helpers.js';
import { plainHeaders } from '../src/adapters/http.js';

const answering = (counts) => (req, res) => {
  counts[req.url] = (counts[req.url] ?? 0) + 1;
  req.resume();
  req.on('end', () => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(req.url === '/rate' ? '{"rate":2}' : '{"ok":true}');
  });
};

async function upstream(counts) {
  return listen(answering(counts));
}

/** The same upstream on IPv6's loopback address, or null on a machine without one. */
async function upstreamSix(counts) {
  const server = http.createServer(answering(counts));
  const listening = await new Promise((resolve) => {
    server.once('error', () => resolve(false));
    server.listen(0, '::1', () => resolve(true));
  });
  if (!listening) return null;
  return {
    port: server.address().port,
    close: () => new Promise((resolve) => {
      server.closeAllConnections();
      server.close(resolve);
    }),
  };
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

test('a fetch answer that sets several cookies is replayed with every one of them', async () => {
  const auth = await listen((req, res) => {
    res.setHeader('set-cookie', ['session=s1; HttpOnly', 'csrf=c1']);
    res.setHeader('content-type', 'application/json');
    res.end('{}');
  });
  process.env.KERN_TEST_AUTH_URL = auth.url;
  try {
    const shop = await production('cookies');
    assert.deepEqual(await (await fetch(shop.url)).json(), { cookies: 2 });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.same, stats.notCompared], [1, 0], JSON.stringify(stats.reasons));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_AUTH_URL;
    await auth.close();
  }
});

test('a node:http call to an address, with a Host header of its own (a virtual host), is replayed under the name it was recorded by', async () => {
  const counts = {};
  const server = await upstream(counts);
  process.env.KERN_TEST_UPSTREAM_PORT = new URL(server.url).port;
  try {
    const shop = await production('vhost', { adapters: ['http'] });
    assert.deepEqual(await (await fetch(`${shop.url}/vhost`)).json(), { rate: 2 });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.same, stats.notCompared], [1, 0], JSON.stringify(stats.reasons));
    assert.deepEqual(counts, { '/rate': 1 }, 'only the real app reached the upstream');
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_UPSTREAM_PORT;
    await server.close();
  }
});

test('a node:http call to an IPv6 address is named with the address in brackets, in the app and in the ghosts alike', async (t) => {
  const counts = {};
  const six = await upstreamSix(counts);
  if (!six) {
    t.skip('this machine has no IPv6 loopback address');
    return;
  }
  process.env.KERN_TEST_UPSTREAM6_PORT = String(six.port);
  try {
    const shop = await production('vhost', { adapters: ['http'] });
    assert.deepEqual(await (await fetch(`${shop.url}/ipv6`)).json(), { rate: 2 });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.same, stats.notCompared], [1, 0], JSON.stringify(stats.reasons));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_UPSTREAM6_PORT;
    await six.close();
  }
});

test('a body chunk Node itself refuses, or takes without a word, does exactly what it does without the twin', async () => {
  const counts = {};
  const server = await upstream(counts);
  process.env.KERN_TEST_UPSTREAM = server.url;
  try {
    const { tryWrites } = await import('./apps/odd-write/old.js');
    // Outside any sampled request, nothing records: these are Node's own answers.
    const alone = tryWrites();
    assert.match(alone[0], /^ERR_INVALID_ARG_TYPE: The "chunk" argument/);
    assert.equal(alone[1], 'written');
    const shop = await production('odd-write', { adapters: ['http'] });
    assert.deepEqual(await (await fetch(shop.url)).json(), { seen: alone });
    await shop.twin.idle();
    assert.equal(shop.twin.stats().skipped, 1, 'a body the recorder could not copy leaves the request skipped');
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_UPSTREAM;
    await server.close();
  }
});

test("headers a node:http call gives as Node's raw list keep their names when the replay passes them on", () => {
  // Node takes headers as an object or as a raw list of names and values; read as an object, a list became headers named '0', '1', ...
  assert.deepEqual(plainHeaders(['X-Trace', 'abc', 'Host', 'shop.example', 'Content-Length', '2', 'x-tag', 'a', 'x-tag', 'b']), { 'x-trace': 'abc', 'x-tag': ['a', 'b'] });
  assert.deepEqual(plainHeaders({ 'X-Trace': 'abc', host: 'shop.example' }), { 'x-trace': 'abc' });
});
