import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { app } from './helpers.js';
import { loadManifest, coveragePlan } from '../src/manifest.js';
import { startGhost } from '../src/ghost.js';

const preload = fileURLToPath(new URL('./apps/preloaded/preload.cjs', import.meta.url));

test('a ghost starts without the app\'s preloads, which would run before its seal', () => {
  const file = join(tmpdir(), `kern-twin-preload-${process.pid}.mjs`);
  writeFileSync(file, `
import { startGhost } from ${JSON.stringify(new URL('../src/ghost.js', import.meta.url).href)};
const ghost = await startGhost(${JSON.stringify(app('preloaded', 'old'))}, { startMs: 20000, memoryMb: 128 });
const job = { start: 1760000000000, seed: 1, request: { method: 'GET', url: '/', headers: {}, body: new Uint8Array() }, io: [] };
const result = await ghost.ask(1, job, 5000);
console.log(JSON.stringify({ app: globalThis.__kernPreloaded === true, ghost: JSON.parse(Buffer.from(result.body).toString()).preloaded }));
await ghost.terminate();
`);
  try {
    const runs = [
      spawnSync(process.execPath, [file], { encoding: 'utf8', timeout: 20000, env: { ...process.env, NODE_OPTIONS: `--require ${JSON.stringify(preload)}` } }),
      spawnSync(process.execPath, ['--require', preload, file], { encoding: 'utf8', timeout: 20000 }),
    ];
    for (const run of runs) {
      assert.equal(run.status, 0, run.stderr);
      assert.deepEqual(JSON.parse(run.stdout), { app: true, ghost: false }, 'the app has its preload; the ghost does not');
    }
  } finally {
    rmSync(file, { force: true });
  }
});

test('a ghost reports, per request, which characters of each changed line ran', async () => {
  const root = fileURLToPath(new URL('./apps/three-prs/base/', import.meta.url));
  const manifest = await loadManifest(fileURLToPath(new URL('./apps/three-prs/manifest.json', import.meta.url)));
  const ghost = await startGhost({ module: join(root, 'src', 'app.js'), export: 'createApp' }, { startMs: 20000, memoryMb: 256, coverage: coveragePlan(manifest, 'old', root) });
  try {
    assert.deepEqual(ghost.loadLines, [], 'no changed line runs while the app loads');
    const ask = (n, value) => ghost.ask(n, {
      start: 1760000000000, seed: 1, io: [], coverage: true,
      request: { method: 'POST', url: '/limit', headers: { 'content-type': 'application/json' }, body: new Uint8Array(Buffer.from(JSON.stringify(value))) },
    }, 5000);
    const given = await ask(1, { limits: { max: 7 } });
    assert.deepEqual(given.lines, { 'old:src/app.js:17': `${'1'.repeat(37)}0000` }, 'the ": 100" branch did not run');
    const blank = await ask(2, { limits: null });
    assert.deepEqual(blank.lines, { 'old:src/app.js:17': `${'1'.repeat(19)}${'0'.repeat(18)}1111` }, 'now only that branch ran');
  } finally {
    await ghost.terminate();
  }
});

test('a ghost keeps at most maxBodyBytes of its answer: past it, only the size and the sha256 cross to the twin\'s thread', async () => {
  const { table } = await import(new URL('./apps/big-answer/new.js', import.meta.url).href);
  const bytes = Buffer.from(JSON.stringify(table()));
  const job = { start: 1760000000000, seed: 1, request: { method: 'GET', url: '/', headers: {}, body: new Uint8Array() }, io: [] };
  for (const kind of ['thread', 'process']) {
    const ghost = await startGhost(app('big-answer', 'new'), { kind, startMs: 20000, memoryMb: 256, maxBodyBytes: 1 << 20 });
    try {
      const result = await ghost.ask(1, job, 5000);
      // Never handed to assert's diff: it would write out all two million bytes.
      assert.ok(result.body === null, `${kind}: the ghost handed over ${result.body?.byteLength} bytes`);
      assert.deepEqual(result.large, { size: bytes.length, digest: createHash('sha256').update(bytes).digest('hex') }, kind);
    } finally {
      await ghost.terminate();
    }
  }
});
