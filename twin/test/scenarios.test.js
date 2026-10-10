import { test } from 'node:test';
import assert from 'node:assert/strict';
import { production } from './helpers.js';

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
