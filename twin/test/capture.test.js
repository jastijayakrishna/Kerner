import { test } from 'node:test';
import assert from 'node:assert/strict';
import { wrapListener } from '../src/capture.js';
import { listen, post, production } from './helpers.js';

async function captureOne(describe, url = '/items/7', headers = {}) {
  let copy;
  const listener = wrapListener((req, res) => res.end('ok'), {
    sample: () => true,
    maxBodyBytes: 1024,
    onError: (error) => {
      throw error;
    },
    describe,
    deliver: (promise) => {
      copy = promise;
    },
  });
  const server = await listen(listener);
  await (await fetch(`${server.url}${url}`, { headers })).text();
  const capture = await copy;
  await server.close();
  return capture;
}

test('a capture names its route, its customer segment and the version that served it', async () => {
  const capture = await captureOne((req) => ({ route: `${req.method} /items/:id`, segment: req.headers['x-plan'] }), '/items/7', { 'x-plan': 'pro' });
  assert.deepEqual([capture.route, capture.segment, capture.serving], ['GET /items/:id', 'pro', 'old']);
  assert.equal(typeof capture.start, 'number');
});

test('a capture records the url and method as the request arrived, even when the app rewrites req.url while routing', async () => {
  // Express strips a mount prefix from req.url while a sub-router handles the request (app.use('/v1', router) leaves req.url as '/crawl')
  // and does not restore it when the router sends the response. The capture must record the url the request arrived with, or a ghost
  // replays the wrong path and the route no longer matches.
  let copy;
  const listener = wrapListener((req, res) => {
    req.url = '/crawl';
    req.method = 'GET';
    res.end('ok');
  }, { sample: () => true, maxBodyBytes: 1024, onError: (error) => { throw error; }, deliver: (promise) => { copy = promise; } });
  const server = await listen(listener);
  await (await fetch(`${server.url}/v1/crawl`, { method: 'POST' })).text();
  const capture = await copy;
  await server.close();
  assert.equal(capture.request.url, '/v1/crawl', JSON.stringify(capture.request));
  assert.equal(capture.request.method, 'POST', JSON.stringify(capture.request));
  assert.equal(capture.route, 'POST /v1/crawl');
});

test('without naming functions a request gets the route of its path; a naming function that throws puts it under "unknown"', async () => {
  const plain = await captureOne(null, '/items/123');
  assert.deepEqual([plain.route, plain.segment], ['GET /items/:id', 'all']);
  const broken = await captureOne(() => {
    throw new Error('no plan here');
  }, '/items/123');
  assert.deepEqual([broken.route, broken.segment], ['GET /items/:id', 'unknown']);
});

test('an aborted request is counted as skipped, never lost', async () => {
  const shop = await production('sleepy');
  await fetch(`${shop.url}/`, { signal: AbortSignal.timeout(50) }).catch(() => {});
  await new Promise((resolve) => setTimeout(resolve, 500));
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.equal(stats.skipped, 1, JSON.stringify(stats));
  assert.equal(stats.reasons['skipped: the request was aborted before its answer finished'], 1, JSON.stringify(stats.reasons));
  await shop.close();
});

test('a body read through events keeps its request: the calls the app makes after reading it are recorded', async () => {
  const pay = await listen((req, res) => {
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true}');
    });
  });
  process.env.KERN_TEST_PAY_URL = pay.url;
  try {
    const shop = await production('events-body');
    assert.deepEqual(await (await post(shop.url, { amount: 25 })).json(), { paid: true });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    // Unrecorded, the charge would be missing from the real run while the old ghost makes it: "not compared" (unfaithful).
    assert.deepEqual([stats.same, stats.notCompared], [1, 0], JSON.stringify(stats));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_PAY_URL;
    await pay.close();
  }
});
