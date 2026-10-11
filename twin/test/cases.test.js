import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { caseName, readCase, runOf, writeCase } from '../src/cases.js';
import { replayCase } from '../src/replay.js';
import { compareRuns } from '../src/compare.js';
import { app } from './helpers.js';

test('a case keeps bytes and dates as they were, and replays a version in a fresh sealed ghost', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-cases-'));
  try {
    const body = new Uint8Array(Buffer.from('{"region":"","limits":null}'));
    const entry = {
      version: 1, candidate: 'sha256:abc', n: 3, time: 1760000000000, route: 'POST /', segment: 'all', adapters: ['fetch'], overlay: false,
      world: { start: 1760000000000, seed: 42 },
      request: { method: 'POST', url: '/', headers: { 'content-type': 'application/json' }, body },
      io: [{ kind: 'pg', key: 'PG x', write: false, text: 'x', values: [new Date(0), new Uint8Array([1, 2])], result: null, error: null }],
      old: { status: 200, headers: { 'content-type': 'application/json' }, body: new Uint8Array(Buffer.from('{"region":"","limit":100}')), effects: [], events: [] },
      new: { status: 0, headers: {}, body: null, effects: [], events: [], error: 'uncaught TypeError: x' },
      verdict: { kinds: ['crash'], details: [] }, lines: {}, pulls: [], cause: null, scrubbed: true, opaque: [], reproduces: true, human: false,
    };
    const file = await writeCase(dir, entry);
    assert.equal(file, join(dir, caseName(entry)));
    assert.deepEqual(readdirSync(dir), [caseName(entry)], 'written whole, nothing left over');
    const back = await readCase(file);
    assert.ok(back.request.body instanceof Uint8Array);
    assert.ok(back.io[0].values[0] instanceof Date);
    assert.deepEqual([...back.io[0].values[1]], [1, 2]);
    const fixed = await replayCase(back, app('blank-field', 'old'));
    assert.equal(compareRuns(runOf(back.old), fixed).outcome, 'same', 'the old version answers as recorded');
    const broken = await replayCase(back, app('blank-field', 'new'));
    assert.ok(compareRuns(runOf(back.old), broken).kinds.includes('crash'));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
