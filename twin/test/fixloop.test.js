import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readCase } from '../src/cases.js';
import { post, shopVersion, threePrs } from './helpers.js';

const cli = fileURLToPath(new URL('../bin/kern-twin.js', import.meta.url));

test('a seeded bug becomes a case, a scripted fix makes the case pass, and the fixed candidate switches on', async () => {
  const root = mkdtempSync(join(tmpdir(), 'kern-fixloop-'));
  const cases = join(root, 'cases');
  // No manifest: this test is about the fix loop, not about changed lines.
  const watched = { manifest: undefined, cases, stateDir: join(root, 'state'), switchOn: 'auto', epsilon: 0.5, minHours: 0 };
  const limit = async (url, limits) => (await post(`${url}/limit`, { limits })).json();
  try {
    // 1. Pull request #103 drops the check for blank limits. The twin finds the bug on the first real request with blank limits, and
    //    the customer still gets the old answer.
    const buggy = await threePrs(watched);
    for (let max = 1; max <= 5; max++) assert.deepEqual(await limit(buggy.url, { max }), { limit: max });
    assert.deepEqual(await limit(buggy.url, null), { limit: 100 });
    await buggy.twin.idle();
    assert.equal(buggy.twin.stats().different, 1);
    assert.equal(buggy.twin.stats().serving, 'old', 'an unexplained difference: it never switches on');
    await buggy.close();
    // 2. The difference became a case: scrubbed, checked again, ready for an agent.
    const [file] = readdirSync(cases);
    const value = await readCase(join(cases, file));
    assert.deepEqual([value.route, value.reproduces, value.human], ['POST /limit', true, false]);
    // 3. The case fails on the buggy candidate and passes on the scripted fix.
    const runCases = (version) => spawnSync(process.execPath, [cli, 'test', '--cases', cases, '--candidate', shopVersion(version).module], { encoding: 'utf8' });
    assert.notEqual(runCases('head').status, 0);
    const passed = runCases('fixed');
    assert.equal(passed.status, 0, passed.stdout + passed.stderr);
    // 4. The fixed candidate passes the known case, proves itself on real requests (blank limits included), and switches on by itself.
    const fixed = await threePrs({ ...watched, candidate: shopVersion('fixed') });
    for (let max = 1; max <= 5; max++) assert.deepEqual(await limit(fixed.url, { max }), { limit: max });
    assert.deepEqual(await limit(fixed.url, null), { limit: 100 });
    await fixed.twin.idle();
    assert.deepEqual(fixed.twin.evidence().known.failing, []);
    assert.equal(fixed.twin.stats().different, 0);
    assert.equal(fixed.twin.stats().serving, 'candidate', 'the known case passed, then six clean comparisons and no unexplained difference');
    await fixed.close();
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
