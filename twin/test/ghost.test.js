import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { app } from './helpers.js';

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
