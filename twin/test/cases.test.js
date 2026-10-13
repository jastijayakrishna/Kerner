import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { caseName, readCase, runOf, writeCase } from '../src/cases.js';
import { replayCase } from '../src/replay.js';
import { compareRuns } from '../src/compare.js';
import { scrubCapture } from '../src/scrub.js';
import { app } from './helpers.js';

test('a recorded answer that set several cookies keeps them as a list: scrubbed one by one, and so in the case file', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-cases-'));
  try {
    const call = {
      kind: 'fetch', key: 'GET https://auth.example/session -', method: 'GET', url: 'https://auth.example/session', write: false, sent: new Uint8Array(),
      status: 200, headers: { 'content-type': 'application/json', 'set-cookie': ['session=s1; HttpOnly', 'csrf=c1'] }, body: new Uint8Array(Buffer.from('{}')), error: null,
    };
    const capture = {
      start: 1, seed: 2, route: 'GET /', segment: 'all', serving: 'old', io: [call], effects: [],
      request: { method: 'GET', url: '/', headers: {}, body: new Uint8Array() }, response: { status: 200, headers: {}, body: new Uint8Array() },
    };
    const { capture: scrubbed } = scrubCapture(capture, {});
    assert.deepEqual(scrubbed.io[0].headers['set-cookie'], ['[scrubbed]', '[scrubbed]']);
    const file = await writeCase(dir, { version: 1, n: 1, time: 1, route: 'GET /', io: scrubbed.io });
    assert.deepEqual((await readCase(file)).io[0].headers['set-cookie'], ['[scrubbed]', '[scrubbed]']);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a case keeps bytes and dates as they were, and replays a version in a fresh sealed ghost', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-cases-'));
  try {
    const body = new Uint8Array(Buffer.from('{"region":"","limits":null}'));
    const entry = {
      version: 1, candidate: 'sha256:abc', n: 3, time: 1760000000000, route: 'POST /', segment: 'all', adapters: ['fetch'], overlay: false, kinds: ['crash'],
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

test('a case that says it reproduces cannot be read unless it holds all it needs to run again; a stub needs none of it', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-cases-'));
  let n = 0;
  const saved = (value) => {
    const file = join(dir, `case-${++n}.json`);
    writeFileSync(file, JSON.stringify(value));
    return file;
  };
  try {
    const whole = {
      version: 1, route: 'POST /', kinds: ['crash'], reproduces: true,
      world: { start: 1760000000000, seed: 42 },
      request: { method: 'POST', url: '/', headers: { 'content-type': 'application/json' }, body: { $bytes: 'e30=' } },
      io: [], old: { status: 200, headers: {}, body: { $bytes: '' } }, new: { status: 500, headers: {}, body: null },
    };
    assert.equal((await readCase(saved(whole))).world.seed, 42);
    const { world, ...worldless } = whole;
    const broken = [
      [worldless, /its world needs a start and a seed/],
      [{ ...whole, world: { start: 1760000000000 } }, /its world needs a start and a seed/],
      [{ ...whole, request: { method: 'POST', url: '/', headers: {} } }, /its request needs a method, a url, headers and body bytes/],
      [{ ...whole, request: { ...whole.request, headers: [] } }, /its request needs a method, a url, headers and body bytes/],
      [{ ...whole, io: null }, /its io must be a list of calls/],
      [{ ...whole, io: [null] }, /its io must be a list of calls/],
      [{ ...whole, io: [{ kind: 'fetch' }] }, /its io must be a list of calls/],
      [{ ...whole, kinds: undefined }, /its kinds must be a list of names/],
      [{ version: 1, route: 'POST /', reproduces: false, why: 'a person must look' }, /its kinds must be a list of names/],
      [{ ...whole, old: undefined }, /it has no old answer/],
      [{ ...whole, new: 'crashed' }, /it has no new answer/],
    ];
    for (const [value, why] of broken) await assert.rejects(readCase(saved(value)), why, JSON.stringify(value).slice(0, 120));
    const stub = await readCase(saved({ version: 1, route: 'POST /', kinds: ['body'], reproduces: false, why: 'a person must look' }));
    assert.equal(stub.reproduces, false, 'a stub holds no request, and is read as it is');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
