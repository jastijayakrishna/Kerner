import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Coordinator } from '../src/coordinator.js';
import { app } from './helpers.js';

const options = (name) => ({ old: app(name, 'old'), candidate: app(name, 'new'), timeoutMs: 3000, startMs: 20000, queueLimit: 10, ghostMemoryMb: 256, maxRestarts: 5, log: null });

/** A request as the real app served it: what came in, what went out, and no outgoing calls. */
function served(url, answer) {
  return {
    start: 1760000000000,
    seed: 7,
    request: { method: 'GET', url, headers: { host: 'shop.example' }, body: '' },
    response: { status: 200, headers: { 'content-type': 'application/json' }, body: Buffer.from(JSON.stringify(answer)) },
    io: [],
    effects: [],
  };
}

test('two ghosts answer a recorded request and the difference is named', async () => {
  const pair = new Coordinator(options('renamed-field'));
  await pair.start();
  assert.equal(pair.disabled, null);
  pair.submit(served('/user', { user_name: 'ada', plan: 'pro' }));
  await pair.idle();
  assert.equal(pair.counts.different, 1);
  const [found] = pair.differences();
  assert.equal(found.url, '/user');
  assert.deepEqual(found.kinds, ['body']);
  assert.deepEqual(found.details, [
    { path: '$.user_name', old: '"ada"', new: 'missing' },
    { path: '$.username', old: 'missing', new: '"ada"' },
  ]);
  await pair.close();
});

test('a change that answers the same is counted as the same', async () => {
  const pair = new Coordinator(options('refactor'));
  await pair.start();
  pair.submit(served('/?n=5', { n: 5, total: 15 }));
  await pair.idle();
  assert.deepEqual([pair.counts.same, pair.counts.different, pair.counts.notCompared], [1, 0, 0]);
  await pair.close();
});

test('running ghosts never keep the app\'s process alive', () => {
  // An app that closes its server expects its process to end; ghosts that are still running must not hold it open.
  // The child runs from a file: ghosts inherit Node's flags, and --input-type (needed for -e) is refused for a ghost's file.
  const file = join(tmpdir(), `kern-twin-alive-${process.pid}.mjs`);
  writeFileSync(file, `
import { Coordinator } from ${JSON.stringify(new URL('../src/coordinator.js', import.meta.url).href)};
const pair = new Coordinator(${JSON.stringify(options('refactor'))});
await pair.start();
console.log(pair.disabled ?? 'running');
`);
  try {
    const out = execFileSync(process.execPath, [file], { encoding: 'utf8', timeout: 15000 });
    assert.equal(out.trim(), 'running');
  } finally {
    rmSync(file, { force: true });
  }
});

test('a ghost\'s output never reaches the app\'s own logs', () => {
  // Ghost logs mixed into production logs would repeat every sampled request's log lines, written by code that served nobody.
  const file = join(tmpdir(), `kern-twin-quiet-${process.pid}.mjs`);
  writeFileSync(file, `
import { Coordinator } from ${JSON.stringify(new URL('../src/coordinator.js', import.meta.url).href)};
const pair = new Coordinator(${JSON.stringify(options('noisy'))});
await pair.start();
pair.submit({
  start: 1760000000000, seed: 7, request: { method: 'GET', url: '/', headers: {}, body: '' },
  response: { status: 200, headers: { 'content-type': 'application/json' }, body: Buffer.from('{"ok":true}') }, io: [], effects: [],
});
await pair.idle();
console.log(JSON.stringify(pair.counts));
await pair.close();
`);
  try {
    const run = spawnSync(process.execPath, [file], { encoding: 'utf8', timeout: 15000 });
    assert.equal(run.status, 0, run.stderr);
    assert.doesNotMatch(run.stdout + run.stderr, /ghost output/);
    assert.equal(JSON.parse(run.stdout).same, 1, run.stdout);
  } finally {
    rmSync(file, { force: true });
  }
});
