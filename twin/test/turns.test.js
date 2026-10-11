import { test } from 'node:test';
import assert from 'node:assert/strict';
import { listen, post, production } from './helpers.js';

/** A payment service that answers everything and counts the charges it is asked for. */
async function payService() {
  const asked = { charge: 0 };
  const service = await listen((req, res) => {
    if (req.url.startsWith('/charge')) asked.charge++;
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true}');
    });
  });
  process.env.KERN_TEST_PAY_URL = service.url;
  return {
    asked,
    close: async () => {
      delete process.env.KERN_TEST_PAY_URL;
      await service.close();
    },
  };
}

test('background work an app starts never mixes into the request a ghost is answering, and never restarts the ghosts', async () => {
  const pay = await payService();
  try {
    const shop = await production('ticker');
    for (let i = 0; i < 5; i++) assert.deepEqual(await (await post(shop.url, { amount: 5 })).json(), { paid: true });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.same, stats.notCompared, stats.different, stats.restarts], [5, 0, 0, 0], JSON.stringify(stats));
    assert.equal(pay.asked.charge, 5, 'only the real app charged');
    await shop.close();
  } finally {
    await pay.close();
  }
});

test('a request keeps its turn through its body\'s events and through its own timers', async () => {
  const pay = await payService();
  try {
    const shop = await production('events-body');
    assert.deepEqual(await (await post(shop.url, { amount: 25 })).json(), { paid: true });
    await shop.twin.idle();
    assert.equal(shop.twin.stats().same, 1, JSON.stringify(shop.twin.stats()));
    await shop.close();
  } finally {
    await pay.close();
  }
  const slow = await production('sleepy');
  assert.deepEqual(await (await fetch(slow.url)).json(), { ok: true });
  await slow.twin.idle();
  assert.equal(slow.twin.stats().same, 1, JSON.stringify(slow.twin.stats()));
  await slow.close();
});
