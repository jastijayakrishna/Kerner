import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { app, get, post, routed, until } from './helpers.js';

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

test('evidence gathered against one old version does not carry over when another old version is deployed', async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'kern-twin-state-'));
  try {
    const first = await routed('refactor', { stateDir });
    for (let n = 1; n <= 5; n++) assert.equal(await get(`${first.url}/?n=${n}`), 200);
    await first.twin.idle();
    await first.close();
    const second = await routed('refactor', { stateDir, old: app('broken', 'old') });
    assert.equal(second.twin.evidence().totals['*'].same, 0, 'the comparisons against the version before prove nothing against this one');
    assert.match(second.twin.evidence().note, /another old version/);
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

test("a new version that cannot load in the app's thread is tried once: the old version keeps serving, the reason says why, and the twin keeps comparing", async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'kern-twin-state-'));
  try {
    const shop = await routed('ghost-only', { stateDir, switchOn: 'auto', epsilon: 0.5, minHours: 0 });
    for (let n = 1; n <= 6; n++) assert.equal(await get(`${shop.url}/?n=${n}`), 200);
    await shop.twin.idle();
    assert.equal(globalThis.kernTwinMainThreadLoads, 1, 'the twin tried to switch it on');
    assert.equal(shop.twin.stats().serving, 'old');
    const control = JSON.parse(readFileSync(join(stateDir, 'control.json'), 'utf8'));
    assert.deepEqual([control.serving, control.hold], ['old', true], 'switching on by itself is held');
    assert.match(control.reason, /the new version could not load in the app's thread: this version cannot start in the app's thread/);
    for (let n = 7; n <= 12; n++) assert.equal(await get(`${shop.url}/?n=${n}`), 200);
    await shop.twin.idle();
    assert.equal(shop.twin.stats().same, 12, 'the twin keeps comparing');
    assert.equal(globalThis.kernTwinMainThreadLoads, 1, 'and does not try again by itself');
    assert.equal(await shop.twin.serve('candidate', 'a person tries again'), false);
    assert.equal(globalThis.kernTwinMainThreadLoads, 2, 'a person may try again');
    assert.equal(shop.twin.stats().serving, 'old');
    await shop.close();
  } finally {
    rmSync(stateDir, { recursive: true, force: true });
  }
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

test('without a state folder, the twin still says why the new version does not serve, and that switching on is held', async () => {
  globalThis.kernTwinMainThreadLoads = 0;
  const shop = await routed('ghost-only');
  try {
    assert.deepEqual([shop.twin.stats().serving, shop.twin.stats().hold], ['old', false]);
    assert.equal(await shop.twin.serve('candidate', 'a person tries'), false);
    const stats = shop.twin.stats();
    assert.deepEqual([stats.serving, stats.hold], ['old', true]);
    assert.match(stats.servingReason, /the new version could not load in the app's thread/);
  } finally {
    await shop.close();
  }
});

test('a new version that never answers a request while it serves is switched back once a customer gives up on it', async () => {
  const shop = await routed('forgets-answer', { timeoutMs: 1000 });
  try {
    assert.equal(await shop.twin.serve('candidate', 'switched on by hand'), true);
    assert.equal(await get(`${shop.url}/report`), 200);
    await assert.rejects(fetch(`${shop.url}/report?weekly=1`, { signal: AbortSignal.timeout(500) }));
    await until(() => shop.twin.stats().serving === 'old', 8000);
    const [found] = shop.twin.differences();
    assert.deepEqual([found.kinds, found.url], [['hang'], '/report?weekly=1'], JSON.stringify(found));
    assert.equal(await get(`${shop.url}/report?weekly=1`), 200, 'the old version answers it again');
  } finally {
    await shop.close();
  }
});
