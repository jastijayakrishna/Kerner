import { test } from 'node:test';
import assert from 'node:assert/strict';
import { app, production, post, listen } from './helpers.js';

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
  assert.deepEqual(found.details, [{ kind: 'body', path: '$.total', old: '4051', new: '4050.9' }]);
  await shop.close();
});

test('new code that starts a program the old code never started is a difference, though it falls back to the same answer', async () => {
  const shop = await production('reach-out');
  assert.deepEqual(await (await fetch(shop.url)).json(), { formats: ['png'] });
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.deepEqual([stats.same, stats.different, stats.notCompared], [0, 1, 0], JSON.stringify(stats));
  const [found] = shop.twin.differences();
  assert.deepEqual(found.kinds, ['refused'], JSON.stringify(found));
  assert.match(found.details[0].new, /starting the program/);
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

test('a bulk import of 300 writes is compared in full, and one extra write in it is caught', async () => {
  const rows = { written: 0 };
  const store = await listen((req, res) => {
    rows.written++;
    req.resume();
    req.on('end', () => {
      res.writeHead(201, { 'content-type': 'application/json' });
      res.end('{"ok":true}');
    });
  });
  process.env.KERN_TEST_ROWS_URL = store.url;
  try {
    const same = await production('bulk', { candidate: app('bulk', 'old') });
    assert.deepEqual(await (await fetch(same.url)).json(), { imported: 300 });
    await same.twin.idle();
    assert.deepEqual([same.twin.stats().same, same.twin.stats().notCompared], [1, 0], JSON.stringify(same.twin.stats().reasons));
    await same.close();
    const twice = await production('bulk');
    assert.deepEqual(await (await fetch(twice.url)).json(), { imported: 300 });
    await twice.twin.idle();
    const [found] = twice.twin.differences();
    assert.deepEqual(found?.kinds, ['effects'], JSON.stringify(twice.twin.stats()));
    assert.match(found.details[0].new, /\(2 times\)$/);
    assert.equal(rows.written, 600, 'only the real app wrote');
    await twice.close();
  } finally {
    delete process.env.KERN_TEST_ROWS_URL;
    await store.close();
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

test('an app that reads its body the way express.json() does is still compared, and its double charge caught', async () => {
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
    const shop = await production('body-events');
    assert.deepEqual(await (await post(shop.url, { amount: 25 })).json(), { paid: true });
    await shop.twin.idle();
    assert.equal(charges, 1);
    const stats = shop.twin.stats();
    assert.equal(stats.notCompared, 0, JSON.stringify(stats));
    const [found] = shop.twin.differences();
    assert.ok(found?.kinds.includes('effects'), JSON.stringify(stats));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_PAY_URL;
    await pay.close();
  }
});

test('a charge whose JSON body is built in another key order is the same write, not a difference', async () => {
  const pay = await listen((req, res) => {
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true}');
    });
  });
  process.env.KERN_TEST_PAY = pay.url;
  try {
    const shop = await production('reordered');
    assert.deepEqual(await (await post(shop.url, { amount: 5, currency: 'usd' })).json(), { charged: true });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.same, stats.different, stats.notCompared], [1, 0, 0], JSON.stringify({ stats, differences: shop.twin.differences() }));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_PAY;
    await pay.close();
  }
});
