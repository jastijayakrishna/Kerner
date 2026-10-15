import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { decide } from '../src/decide.js';
import { app, post, production, routed } from './helpers.js';

const watched = (cases) => ({ cases, switchOn: 'auto', epsilon: 0.5, minHours: 0 });
const reasons = (twin) => twin.decision().reasons.join('; ');
const order = (card) => ({ email: 'ada@example.com', ...(card ? { card: '4111 1111 1111 1111' } : {}) });

test('decide holds a candidate while a known case is unchecked, still differs, needs a person, or cannot be read', () => {
  const counts = { same: 6, stated: 0, different: 0, notCompared: 0, skipped: 0, first: 1, last: 1 };
  const snapshot = { since: 1, segments: { all: { 'GET /': counts } }, unexplained: [], lines: {}, loadLines: [] };
  const policy = { epsilon: 0.5, minHours: 0, segments: ['*'], maxUncovered: 0.05 };
  const known = (more) => ({ pending: 0, failing: [], unchecked: [], stubs: [], unreadable: [], ...more });
  assert.equal(decide(snapshot, { ...policy, known: known({ unchecked: ['a.json'] }) }, Date.now()).ready, true, 'a case the ghosts can no longer compare does not hold it');
  assert.deepEqual(decide(snapshot, { ...policy, known: known({ pending: 2, failing: ['b.json'], stubs: ['c.json'], unreadable: ['d.json'] }) }, Date.now()).reasons, [
    '2 known cases not yet checked',
    '1 known case still shows a difference: b.json',
    '1 known difference needs a person, who removes its file once done: c.json',
    '1 known case file could not be read: d.json',
  ]);
});

test('a later candidate that still shows a known difference stays off, and one that fixed it switches on', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-known-'));
  const cases = join(dir, 'cases');
  const clean = async (url) => {
    for (let i = 0; i < 6; i++) await (await post(url, order(false))).arrayBuffer();
  };
  try {
    const first = await routed('checkout', watched(cases));
    await (await post(first.url, order(true))).arrayBuffer();
    await first.twin.idle();
    assert.equal(first.twin.stats().different, 1);
    await first.close();
    assert.equal(readdirSync(cases).length, 1, 'the difference became a case');
    const again = await routed('checkout', watched(cases));
    await clean(again.url);
    await again.twin.idle();
    assert.deepEqual(again.twin.evidence().known.failing, readdirSync(cases));
    assert.equal(again.twin.stats().serving, 'old');
    assert.match(reasons(again.twin), /1 known case still shows a difference/);
    await again.close();
    const fixed = await routed('checkout', { ...watched(cases), candidate: app('checkout', 'fixed') });
    await clean(fixed.url);
    await fixed.twin.idle();
    assert.deepEqual(fixed.twin.evidence().known, { pending: 0, failing: [], unchecked: [], stubs: [], unreadable: [] });
    assert.equal(fixed.twin.stats().serving, 'candidate', 'the known case passed, and six clean comparisons followed');
    await fixed.close();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a known difference only a person can judge, or a case file that cannot be read, holds every later candidate until it is removed', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-known-'));
  const later = () => routed('emails', { ...watched(dir), candidate: app('emails', 'old') });
  const clean = async (url) => {
    for (let i = 0; i < 6; i++) await (await post(url, { email: `u${i}@example.com` })).arrayBuffer();
  };
  const settle = async (shop) => {
    await clean(shop.url);
    await shop.twin.idle();
  };
  try {
    const first = await production('emails', { cases: dir });
    await (await post(first.url, { email: 'ada@corp.com' })).arrayBuffer();
    await first.twin.idle();
    await first.close();
    const [stub] = readdirSync(dir);
    const held = await later();
    await settle(held);
    assert.equal(held.twin.stats().serving, 'old');
    assert.match(reasons(held.twin), /1 known difference needs a person/);
    await held.close();
    rmSync(join(dir, stub));
    writeFileSync(join(dir, '1-1-broken.json'), '{');
    const broken = await later();
    await settle(broken);
    assert.equal(broken.twin.stats().serving, 'old');
    assert.match(reasons(broken.twin), /1 known case file could not be read: 1-1-broken\.json/);
    await broken.close();
    rmSync(join(dir, '1-1-broken.json'));
    const cleared = await later();
    await settle(cleared);
    assert.equal(cleared.twin.stats().serving, 'candidate');
    await cleared.close();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a case file that says it reproduces but holds no request is listed as unreadable and holds the candidate, and the twin stays on', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-known-'));
  try {
    writeFileSync(join(dir, '1-1-post.json'), JSON.stringify({ version: 1, route: 'POST /', kinds: ['body'], reproduces: true }));
    const shop = await routed('emails', { ...watched(dir), candidate: app('emails', 'old') });
    for (let i = 0; i < 6; i++) await (await post(shop.url, { email: `u${i}@example.com` })).arrayBuffer();
    await shop.twin.idle();
    assert.equal(shop.twin.stats().disabled, null, 'one broken file does not switch the twin off');
    assert.deepEqual(shop.twin.evidence().known.unreadable, ['1-1-post.json']);
    assert.equal(shop.twin.decision().ready, false);
    assert.match(reasons(shop.twin), /1 known case file could not be read: 1-1-post\.json/);
    assert.equal(shop.twin.stats().serving, 'old');
    assert.equal(shop.twin.stats().same, 6, 'and the twin keeps comparing');
    await shop.close();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('twin.idle() waits for the known cases to be checked, and for the switch the last check allows', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-known-'));
  const cases = join(dir, 'cases');
  const state = join(dir, 'state');
  try {
    const first = await routed('checkout', watched(cases));
    for (let i = 0; i < 2; i++) await (await post(first.url, order(true))).arrayBuffer();
    await first.twin.idle();
    await first.close();
    assert.equal(readdirSync(cases).length, 2);
    const earning = await routed('checkout', { stateDir: state, candidate: app('checkout', 'fixed') });
    for (let i = 0; i < 6; i++) await (await post(earning.url, order(false))).arrayBuffer();
    await earning.twin.idle();
    await earning.close();
    const fixed = await routed('checkout', { ...watched(cases), stateDir: state, candidate: app('checkout', 'fixed') });
    await (await post(fixed.url, order(false))).arrayBuffer();
    await fixed.twin.idle();
    assert.equal(fixed.twin.evidence().known.pending, 0, 'idle once both known cases were checked');
    assert.equal(fixed.twin.stats().serving, 'candidate', 'and once the switch the last check allowed was made');
    await fixed.close();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
