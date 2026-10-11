import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { get, routed } from './helpers.js';

const cli = fileURLToPath(new URL('../bin/kern-twin.js', import.meta.url));
const run = (...args) => JSON.parse(execFileSync(process.execPath, [cli, ...args], { encoding: 'utf8' }));

test('kern-twin status shows the evidence and the decision; kern-twin serve switches by hand', async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'kern-twin-cli-'));
  try {
    const shop = await routed('refactor', { stateDir });
    for (let n = 1; n <= 6; n++) await get(`${shop.url}/?n=${n}`);
    await shop.twin.idle();
    await shop.close();
    const status = run('status', '--state', stateDir, '--epsilon', '0.5', '--min-hours', '0');
    assert.deepEqual([status.serving, status.hold, status.totals['*'].same, status.decision.ready], ['old', false, 6, true]);
    assert.match(status.candidate, /^sha256:/);
    const served = run('serve', 'candidate', '--state', stateDir, '--reason', 'checked by a person');
    assert.deepEqual([served.serving, served.hold, served.reason], ['candidate', false, 'checked by a person']);
    assert.equal(JSON.parse(readFileSync(join(stateDir, 'control.json'), 'utf8')).serving, 'candidate');
    const refused = spawnSync(process.execPath, [cli, 'serve', 'sideways', '--state', stateDir], { encoding: 'utf8' });
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /serve takes 'old' or 'candidate'/);
  } finally {
    rmSync(stateDir, { recursive: true, force: true });
  }
});
