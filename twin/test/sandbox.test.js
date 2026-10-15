import { test } from 'node:test';
import assert from 'node:assert/strict';
import { post, production, threePrs } from './helpers.js';

const SANDBOX = process.env.KERN_TWIN_SANDBOX === '1';
const skip = !SANDBOX && 'needs bubblewrap: run scripts/test-sandbox.sh';

test('in the bwrap sandbox a ghost has no network, even past its JavaScript seal', { skip }, async (t) => {
  const keep = new URL('./apps/escape-net/keep.mjs', import.meta.url).href;
  const shop = await production('escape-net', { ghosts: 'process', sandbox: 'bwrap', ghostExecArgv: ['--import', keep] });
  t.after(() => shop.close());
  assert.equal(shop.twin.stats().disabled, null, shop.twin.stats().disabled);
  await (await fetch(shop.url)).arrayBuffer();
  await shop.twin.idle();
  const [found] = shop.twin.differences();
  assert.ok(found.details.some((d) => d.path === '$.reached' && d.new === '"ENETUNREACH"'), JSON.stringify(found));
});

test('in the bwrap sandbox the changed lines are still measured', { skip }, async (t) => {
  const shop = await threePrs({ ghosts: 'process', sandbox: 'bwrap' });
  t.after(() => shop.close());
  assert.equal(shop.twin.stats().disabled, null, shop.twin.stats().disabled);
  await (await post(`${shop.url}/limit`, { limits: { max: 3 } })).arrayBuffer();
  await shop.twin.idle();
  assert.equal(shop.twin.paths().find((p) => p.key === 'old:src/app.js:17').status, 'partly run');
});
