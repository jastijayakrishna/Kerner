import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readCase } from '../src/cases.js';
import { post, production, threePrs } from './helpers.js';

const SECRETS = ['ada@corp.com', '4111 1111 1111 1111', 'sk_live_0123456789abcdefghij0123456789ab'];

test('an unexplained difference becomes a scrubbed case that still shows it, and nothing private reaches the disk', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-casefiles-'));
  try {
    const shop = await production('checkout', { cases: join(dir, 'cases'), log: join(dir, 'differences.jsonl'), stateDir: join(dir, 'state') });
    // The email is in the address too, where a route name could carry it to the disk.
    const answer = await fetch(`${shop.url}/orders/${SECRETS[0]}`, {
      method: 'POST',
      headers: { authorization: `Bearer ${SECRETS[2]}`, 'content-type': 'application/json' },
      body: JSON.stringify({ email: SECRETS[0], card: SECRETS[1] }),
    });
    assert.deepEqual(await answer.json(), { charged: true, receipt: SECRETS[0] }, 'the customer was served by the old version');
    await shop.twin.idle();
    assert.ok(shop.twin.differences()[0].details.some((d) => String(d.new).includes(SECRETS[1])), 'in memory the real values stay, for the operator');
    await shop.close();
    const [file] = readdirSync(join(dir, 'cases'));
    assert.doesNotMatch(file, /ada|corp/, 'nor in the name of the case file');
    const written = [readFileSync(join(dir, 'cases', file), 'utf8'), readFileSync(join(dir, 'differences.jsonl'), 'utf8'), readFileSync(join(dir, 'state', 'evidence.json'), 'utf8')].join('\n');
    for (const secret of SECRETS) assert.ok(!written.includes(secret), `${secret} reached the disk`);
    const value = await readCase(join(dir, 'cases', file));
    assert.deepEqual([value.reproduces, value.human, value.route], [true, false, 'POST /orders/:id']);
    assert.ok(value.verdict.kinds.includes('crash'), JSON.stringify(value.verdict));
    assert.match(value.new.error, /cannot charge 4\d{3} \d{4} \d{4} \d{4}/, 'the case shows the stand-in card number');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a difference that depends on a private value becomes a stub a person must look at, with no request data', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-casefiles-'));
  try {
    const shop = await production('emails', { cases: dir });
    assert.deepEqual(await (await post(shop.url, { email: 'ada@corp.com' })).json(), { domain: 'corp.com' });
    await shop.twin.idle();
    assert.equal(shop.twin.stats().different, 1);
    await shop.close();
    const [file] = readdirSync(dir);
    const value = await readCase(join(dir, file));
    assert.equal(value.reproduces, false);
    assert.equal(value.request, undefined);
    assert.match(value.why, /may depend on a private value/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a scrubbed copy the app answers with another status, such as a sign-in it can no longer read, becomes a stub, not a case', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-casefiles-'));
  process.env.KERN_TEST_TOKEN = 'token-for-the-test';
  try {
    const shop = await production('auth-shop', { cases: dir });
    assert.deepEqual(await (await fetch(shop.url, { headers: { authorization: 'Bearer token-for-the-test' } })).json(), { plan: 'pro' });
    await shop.twin.idle();
    assert.equal(shop.twin.stats().different, 1);
    await shop.close();
    const [file] = readdirSync(dir);
    const value = await readCase(join(dir, file));
    // The scrubbed copies differ too (their refusals are worded differently), but they are refusals, not the plans that differed.
    assert.equal(value.reproduces, false, JSON.stringify(value.verdict ?? value.why));
    assert.match(value.why, /another status/);
  } finally {
    delete process.env.KERN_TEST_TOKEN;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('running both ghosts again for a case leaves no changed-line counts behind for the next request', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-casefiles-'));
  try {
    // With the manifest, each ghost counts the changed lines it runs. The profile change renames a field: a difference without a crash,
    // so the ghosts are not restarted after it.
    const shop = await threePrs({ cases: dir });
    await (await post(`${shop.url}/profile`, { name: 'Ada', plan: 'pro' })).arrayBuffer();
    await (await post(`${shop.url}/total`, { items: [{ price: 2, count: 3 }] })).arrayBuffer();
    await shop.twin.idle();
    assert.deepEqual([shop.twin.stats().different, shop.twin.stats().same], [1, 1]);
    assert.equal(readdirSync(dir).length, 1, 'the profile difference became a case: both ghosts ran its changed line once more');
    const touched = (key) => shop.twin.paths().find((p) => p.key === key).touched;
    assert.deepEqual([touched('old:src/app.js:13'), touched('new:src/app.js:13')], [1, 1], 'only the request to /profile ran the profile line; the clean request to /total is not credited with it');
    await shop.close();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
