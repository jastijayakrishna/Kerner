import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { app, production } from './helpers.js';

const cli = fileURLToPath(new URL('../bin/kern-twin.js', import.meta.url));

test('kern-twin test fails while the new version still differs on a case, and passes once a fix answers as the old one did', async () => {
  const cases = mkdtempSync(join(tmpdir(), 'kern-casetest-'));
  try {
    const shop = await production('checkout', { cases });
    await (await fetch(shop.url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: 'ada@corp.com', card: '4111 1111 1111 1111' }) })).arrayBuffer();
    await shop.twin.idle();
    await shop.close();
    const run = (version) => spawnSync(process.execPath, [cli, 'test', '--cases', cases, '--candidate', app('checkout', version).module], { encoding: 'utf8' });
    const broken = run('new');
    assert.notEqual(broken.status, 0, broken.stdout);
    assert.match(broken.stdout, /crash/);
    const fixed = run('fixed');
    assert.equal(fixed.status, 0, fixed.stdout + fixed.stderr);
    assert.match(fixed.stdout, /# pass 1/);
  } finally {
    rmSync(cases, { recursive: true, force: true });
  }
});

test('a case file kern-twin test cannot read fails as its own test, named, and every other case still runs', async () => {
  const cases = mkdtempSync(join(tmpdir(), 'kern-casetest-'));
  try {
    const shop = await production('checkout', { cases });
    await (await fetch(shop.url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: 'ada@corp.com', card: '4111 1111 1111 1111' }) })).arrayBuffer();
    await shop.twin.idle();
    await shop.close();
    writeFileSync(join(cases, '0-0-broken.json'), '{ not json');
    const run = spawnSync(process.execPath, [cli, 'test', '--cases', cases, '--candidate', app('checkout', 'fixed').module], { encoding: 'utf8' });
    assert.notEqual(run.status, 0, 'a case that cannot be read never passes quietly');
    assert.match(run.stdout, /not ok \d+ - 0-0-broken\.json/, run.stdout);
    assert.match(run.stdout, /# pass 1/, run.stdout);
    assert.match(run.stdout, /# fail 1/, run.stdout);
  } finally {
    rmSync(cases, { recursive: true, force: true });
  }
});
