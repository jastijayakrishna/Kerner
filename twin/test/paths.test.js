import { test } from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../src/index.js';
import { get, listen, post, shopVersion, threePrs } from './helpers.js';

const status = (twin, key) => twin.paths().find((p) => p.key === key)?.status;

/** One version of the plans shop, whose change adds a plan written with a brace alone on its line, and the manifest of that change. */
const plans = (version) => {
  const root = fileURLToPath(new URL(`./apps/plans/${version}/`, import.meta.url));
  return { module: join(root, 'src', 'app.js'), export: 'createApp', root };
};
const plansManifest = fileURLToPath(new URL('./apps/plans/manifest.json', import.meta.url));

test('a change whose old branch no request has taken stays off, and that line is listed, until a request takes it', async () => {
  const shop = await threePrs({
    switchOn: 'auto', epsilon: 0.5, minHours: 0, minLineRuns: 1,
    accept: ['old:src/app.js:9', 'new:src/app.js:9', 'old:src/app.js:13', 'new:src/app.js:13'],
  });
  for (let max = 1; max <= 6; max++) assert.deepEqual(await (await post(`${shop.url}/limit`, { limits: { max } })).json(), { limit: max });
  await shop.twin.idle();
  assert.equal(shop.twin.stats().same, 6);
  assert.equal(status(shop.twin, 'old:src/app.js:17'), 'partly run', 'the ": 100" branch, which blank limits take, has never run');
  assert.equal(status(shop.twin, 'new:src/app.js:17'), 'proven');
  assert.equal(status(shop.twin, 'new:src/app.js:9'), 'accepted');
  const decision = shop.twin.decision();
  assert.equal(decision.ready, false);
  assert.deepEqual(decision.waiting, ['old:src/app.js:17']);
  assert.equal(shop.twin.stats().serving, 'old', 'six clean comparisons are not enough while a changed path has never run');
  // The first request with blank limits takes that path: the old code answers it, the new one would crash, and the change stays off.
  assert.deepEqual(await (await post(`${shop.url}/limit`, { limits: null })).json(), { limit: 100 });
  await shop.twin.idle();
  const [found] = shop.twin.differences();
  assert.ok(found.kinds.includes('crash'), JSON.stringify(found));
  assert.equal(found.cause, 103);
  assert.equal(shop.twin.stats().serving, 'old');
  await shop.close();
});

test('each difference names the pull request whose changed lines it ran', async () => {
  const shop = await threePrs();
  assert.deepEqual(await (await post(`${shop.url}/total`, { items: [{ price: 0.1, count: 3 }] })).json(), { total: 0.30000000000000004 });
  assert.deepEqual(await (await post(`${shop.url}/profile`, { name: 'ada', plan: 'pro' })).json(), { user_name: 'ada', plan: 'pro' });
  assert.deepEqual(await (await post(`${shop.url}/limit`, { limits: null })).json(), { limit: 100 });
  await shop.twin.idle();
  const found = Object.fromEntries(shop.twin.differences().map((d) => [d.url, [d.cause, d.pulls]]));
  assert.deepEqual(found, { '/total': [101, [101]], '/profile': [102, [102]], '/limit': [103, [103]] });
  await shop.close();
});

test('a changed line with no code on it, such as a brace alone, has nothing to prove and holds nothing back', async () => {
  const twin = await createTwin({ old: plans('base'), candidate: plans('head'), manifest: plansManifest, timeoutMs: 3000, switchOn: 'auto', epsilon: 0.5, minHours: 0, minLineRuns: 1 });
  const server = await listen(await twin.listener());
  try {
    for (let i = 0; i < 6; i++) assert.equal(await get(`${server.url}/?plan=free`), 200);
    await twin.idle();
    assert.deepEqual(twin.paths().map((p) => [p.key, p.status]), [['new:src/app.js:7', 'no code'], ['new:src/app.js:8', 'at load'], ['new:src/app.js:9', 'at load']]);
    assert.deepEqual(twin.decision().waiting, []);
    assert.equal(twin.stats().serving, 'candidate', 'six clean comparisons, and every changed line ran at load or has no code');
  } finally {
    await server.close();
    await twin.close();
  }
});

test('a manifest that does not describe the code switches the twin off and names the line, and customers are served as always', async () => {
  const shop = await threePrs({ old: shopVersion('head') });
  assert.match(shop.twin.stats().disabled, /the old code at .* is not what the manifest describes: src\/app\.js:9/);
  assert.deepEqual(await (await post(`${shop.url}/limit`, { limits: { max: 2 } })).json(), { limit: 2 });
  await shop.close();
});
