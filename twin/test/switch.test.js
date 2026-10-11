import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { get, post, routed, until } from './helpers.js';

test('with switchOn auto, the new version starts serving after exactly ceil(3/ε) clean comparisons', async () => {
  const shop = await routed('refactor', { switchOn: 'auto', epsilon: 0.1, minHours: 0 });
  for (let n = 1; n <= 29; n++) assert.equal(await get(`${shop.url}/?n=${n}`), 200);
  await shop.twin.idle();
  assert.equal(shop.twin.stats().serving, 'old');
  assert.deepEqual(shop.twin.decision().reasons, ['all traffic: 29 of 30 clean comparisons']);
  assert.equal(await get(`${shop.url}/?n=30`), 200);
  await shop.twin.idle();
  assert.equal(shop.twin.stats().serving, 'candidate');
  assert.equal(shop.twin.decision().ready, true);
  await shop.close();
});

test('a regression while the new version serves switches serving back before the next request', async () => {
  const shop = await routed('yen');
  assert.equal(await shop.twin.serve('candidate', 'a person switched it on'), true);
  assert.deepEqual(await (await post(shop.url, { amount: 10, currency: 'EUR' })).json(), { currency: 'EUR', total: 9 });
  assert.deepEqual(await (await post(shop.url, { amount: 4501, currency: 'JPY' })).json(), { currency: 'JPY', total: 4050.9 }, 'the new version served this customer');
  await shop.twin.idle();
  assert.equal(shop.twin.stats().serving, 'old');
  assert.deepEqual(await (await post(shop.url, { amount: 4501, currency: 'JPY' })).json(), { currency: 'JPY', total: 4051 }, 'the next customer gets the old answer');
  await shop.close();
});

test('a person switches serving with the control file, and a restart keeps the switch and the evidence', async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'kern-twin-state-'));
  try {
    const first = await routed('refactor', { stateDir });
    for (let n = 1; n <= 5; n++) assert.equal(await get(`${first.url}/?n=${n}`), 200);
    await first.twin.idle();
    assert.equal(first.twin.evidence().totals['*'].same, 5);
    const file = join(stateDir, 'control.json');
    const control = JSON.parse(readFileSync(file, 'utf8'));
    assert.deepEqual([control.serving, control.hold], ['old', false]);
    writeFileSync(file, JSON.stringify({ ...control, serving: 'candidate', reason: 'a person switched it on' }));
    await until(() => first.twin.stats().serving === 'candidate');
    await first.close();
    const second = await routed('refactor', { stateDir });
    assert.equal(second.twin.stats().serving, 'candidate', 'the switch survives a restart');
    assert.equal(second.twin.evidence().totals['*'].same, 5, 'so do the counts');
    assert.equal(second.twin.evidence().note, null);
    await second.close();
  } finally {
    rmSync(stateDir, { recursive: true, force: true });
  }
});

test('if the twin stops while the new version serves, the old version serves again', async () => {
  process.env.KERN_TWIN_FAULT = 'thread-exit';
  try {
    const shop = await routed('yen');
    await shop.twin.serve('candidate', 'a person switched it on');
    assert.deepEqual(await (await post(shop.url, { amount: 10, currency: 'EUR' })).json(), { currency: 'EUR', total: 9 });
    await until(() => shop.twin.stats().disabled !== null);
    await shop.twin.idle();
    assert.match(shop.twin.stats().disabled, /the twin's thread stopped/);
    assert.equal(shop.twin.stats().serving, 'old');
    assert.deepEqual(await (await post(shop.url, { amount: 4501, currency: 'JPY' })).json(), { currency: 'JPY', total: 4051 });
    await shop.close();
  } finally {
    delete process.env.KERN_TWIN_FAULT;
  }
});

test('a new version that cannot load is never switched on, and the old one keeps serving', async () => {
  const shop = await routed('broken');
  assert.match(shop.twin.stats().disabled, /could not start/);
  assert.equal(await shop.twin.serve('candidate', 'try anyway'), false);
  assert.equal(shop.twin.stats().serving, 'old');
  assert.deepEqual(await (await fetch(`${shop.url}/?n=3`)).json(), { n: 3, total: 6 });
  await shop.close();
});

test('a person who switches the new version off keeps it off, even where it would switch on by itself', async () => {
  const shop = await routed('refactor', { switchOn: 'auto', epsilon: 0.5, minHours: 0 });
  for (let n = 1; n <= 6; n++) await get(`${shop.url}/?n=${n}`);
  await shop.twin.idle();
  assert.equal(shop.twin.stats().serving, 'candidate');
  assert.equal(await shop.twin.serve('old', 'a person wants to look first'), true);
  for (let n = 7; n <= 9; n++) await get(`${shop.url}/?n=${n}`);
  await shop.twin.idle();
  assert.equal(shop.twin.stats().serving, 'old');
  await shop.close();
});
