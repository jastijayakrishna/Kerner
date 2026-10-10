import { test } from 'node:test';
import assert from 'node:assert/strict';
import { production, post, listen } from './helpers.js';

test('a change that behaves the same is reported as the same', async () => {
  const shop = await production('refactor');
  assert.deepEqual(await (await fetch(`${shop.url}/?n=5`)).json(), { n: 5, total: 15 });
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.deepEqual([stats.same, stats.different, stats.notCompared, stats.skipped], [1, 0, 0, 0], JSON.stringify(stats));
  await shop.close();
});

test('a renamed field is caught while the customer gets the old answer', async () => {
  const shop = await production('renamed-field');
  assert.deepEqual(await (await fetch(`${shop.url}/user`)).json(), { user_name: 'ada', plan: 'pro' });
  await shop.twin.idle();
  const [found] = shop.twin.differences();
  assert.equal(found.method, 'GET');
  assert.equal(found.url, '/user');
  assert.ok(found.details.some((d) => d.path === '$.username' && d.old === 'missing'), JSON.stringify(found));
  await shop.close();
});

test('a crash on data only some requests carry is caught, and the customer still gets the old answer', async () => {
  const shop = await production('blank-field');
  assert.deepEqual(await (await post(shop.url, { region: 'eu', limits: { max: 7 } })).json(), { region: 'eu', limit: 7 });
  const answer = await post(shop.url, { region: '', limits: null });
  assert.equal(answer.status, 200);
  assert.deepEqual(await answer.json(), { region: '', limit: 100 });
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.deepEqual([stats.same, stats.different], [1, 1], JSON.stringify(stats));
  const [found] = shop.twin.differences();
  assert.ok(found.kinds.includes('crash'), JSON.stringify(found));
  assert.match(found.details[0].new, /TypeError/);
  await shop.close();
});

test('a rounding change on one currency is caught with the exact field', async () => {
  const shop = await production('yen');
  assert.deepEqual(await (await post(shop.url, { amount: 10, currency: 'EUR' })).json(), { currency: 'EUR', total: 9 });
  assert.deepEqual(await (await post(shop.url, { amount: 4501, currency: 'JPY' })).json(), { currency: 'JPY', total: 4051 });
  await shop.twin.idle();
  assert.equal(shop.twin.stats().same, 1);
  const [found] = shop.twin.differences();
  assert.deepEqual(found.kinds, ['body']);
  assert.deepEqual(found.details, [{ path: '$.total', old: '4051', new: '4050.9' }]);
  await shop.close();
});

test('a route that refused a stranger and would now let them in is a door that opened', async () => {
  const shop = await production('open-door');
  assert.equal((await fetch(`${shop.url}/admin`)).status, 401);
  await shop.twin.idle();
  const [found] = shop.twin.differences();
  assert.ok(found.kinds.includes('door-opened'), JSON.stringify(found));
  await shop.close();
});

test('a change that would charge twice is caught without charging anyone twice', async () => {
  let charges = 0;
  const pay = await listen((req, res) => {
    charges++;
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true}');
    });
  });
  process.env.KERN_TEST_PAY_URL = pay.url;
  try {
    const shop = await production('double-charge');
    assert.deepEqual(await (await post(shop.url, { amount: 25 })).json(), { paid: true });
    await shop.twin.idle();
    assert.equal(charges, 1);
    const [found] = shop.twin.differences();
    assert.ok(found.kinds.includes('effects'), JSON.stringify(found));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_PAY_URL;
    await pay.close();
  }
});

test('time and random numbers never count as differences', async () => {
  const shop = await production('timestamps');
  await (await fetch(`${shop.url}/`)).arrayBuffer();
  await (await fetch(`${shop.url}/`)).arrayBuffer();
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.deepEqual([stats.same, stats.different, stats.notCompared], [2, 0, 0], JSON.stringify(stats));
  await shop.close();
});
