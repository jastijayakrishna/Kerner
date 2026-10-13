import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Evidence, canonical } from '../src/evidence.js';
import { T, verdict } from './verdicts.js';

test('evidence counts every outcome per segment and route, and keeps the first unexplained differences', () => {
  const evidence = new Evidence({ candidate: 'c1' });
  for (const outcome of ['same', 'same', 'stated', 'not-compared', 'skipped']) evidence.add(verdict(outcome));
  evidence.add(verdict('different', { kinds: ['body'], details: [{ kind: 'body', path: '$.total', old: '1', new: '2' }] }));
  evidence.add(verdict('same', { segment: 'pro', route: 'POST /orders', time: T + 1000 }));
  const data = evidence.snapshot();
  assert.deepEqual(data.segments.all['GET /users/:id'], { same: 2, stated: 1, different: 1, notCompared: 1, skipped: 1, first: T, last: T });
  assert.deepEqual(data.segments.pro['POST /orders'], { same: 1, stated: 0, different: 0, notCompared: 0, skipped: 0, first: T + 1000, last: T + 1000 });
  assert.equal(data.since, T);
  assert.equal(data.unexplained.length, 1);
  assert.deepEqual(data.unexplained[0].details, [{ kind: 'body', path: '$.total', old: '1', new: '2' }]);
});

test('evidence survives a restart; a damaged file or another candidate\'s file starts again and says why', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-evidence-'));
  const file = join(dir, 'evidence.json');
  try {
    const first = await Evidence.open({ file, candidate: 'c1' });
    first.add(verdict('same'));
    first.add(verdict('same'));
    await first.flush();
    const again = await Evidence.open({ file, candidate: 'c1' });
    assert.equal(again.note, null);
    assert.equal(again.snapshot().segments.all['GET /users/:id'].same, 2);
    const other = await Evidence.open({ file, candidate: 'c2' });
    assert.match(other.note, /another candidate \(c1\)/);
    assert.deepEqual(other.snapshot().segments, {});
    const kept = await Evidence.open({ file, candidate: 'c1' });
    kept.add(verdict('different', { url: undefined, kinds: ['body'] })); // a value JSON drops must not break the checksum
    await kept.flush();
    assert.equal((await Evidence.open({ file, candidate: 'c1' })).note, null);
    const stored = JSON.parse(readFileSync(file, 'utf8'));
    stored.segments.all['GET /users/:id'].same = 3000; // a hand edit that keeps the old checksum
    writeFileSync(file, JSON.stringify(stored));
    const damaged = await Evidence.open({ file, candidate: 'c1' });
    assert.match(damaged.note, /checksum/);
    assert.deepEqual(damaged.snapshot().segments, {});
    writeFileSync(file, '{ not json');
    assert.match((await Evidence.open({ file, candidate: 'c1' })).note, /not JSON/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('evidence gathered against one old version starts again against another, and its note names the old version', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-evidence-'));
  const file = join(dir, 'evidence.json');
  try {
    const first = await Evidence.open({ file, candidate: 'c1', old: 'sha256:old-a' });
    for (let i = 0; i < 3; i++) first.add(verdict('same'));
    await first.flush();
    const again = await Evidence.open({ file, candidate: 'c1', old: 'sha256:old-a' });
    assert.equal(again.note, null);
    assert.equal(again.snapshot().segments.all['GET /users/:id'].same, 3);
    // A hotfix deployed as the old version: comparisons against the version before it prove nothing about the candidate against this one.
    const hotfixed = await Evidence.open({ file, candidate: 'c1', old: 'sha256:old-b' });
    assert.match(hotfixed.note, /another old version \(sha256:old-a\).*sha256:old-b/);
    assert.deepEqual(hotfixed.snapshot().segments, {});
    // A file written before evidence named its old version is for an old version no one knows: it starts again too.
    const { sha256, old, ...earlier } = JSON.parse(readFileSync(file, 'utf8'));
    writeFileSync(file, JSON.stringify({ ...earlier, sha256: createHash('sha256').update(canonical(earlier)).digest('hex') }));
    const unnamed = await Evidence.open({ file, candidate: 'c1', old: 'sha256:old-a' });
    assert.match(unnamed.note, /another old version/);
    assert.deepEqual(unnamed.snapshot().segments, {});
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('evidence that cannot be written is counted, leaves nothing behind, and is written at the next try', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-evidence-'));
  const file = join(dir, 'evidence.json');
  try {
    writeFileSync(join(dir, 'blocker'), 'a file where a folder should be');
    const blocked = await Evidence.open({ file: join(dir, 'blocker', 'evidence.json'), candidate: 'c1' });
    blocked.add(verdict('same'));
    await blocked.flush();
    assert.equal(blocked.errors, 1);
    assert.deepEqual(readdirSync(dir), ['blocker'], 'no temporary file left behind');
    const evidence = await Evidence.open({ file, candidate: 'c1' });
    evidence.add(verdict('same'));
    await evidence.flush();
    evidence.add(verdict('same'));
    await evidence.flush();
    assert.equal(JSON.parse(readFileSync(file, 'utf8')).segments.all['GET /users/:id'].same, 2);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a flush resolves only once every write asked for before it, and its own, are on disk', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-evidence-'));
  const file = join(dir, 'evidence.json');
  const earlier = [];
  try {
    const evidence = await Evidence.open({ file, candidate: 'c1' });
    evidence.add(verdict('same'));
    earlier.push(evidence.flush());
    evidence.add(verdict('same', { route: 'POST /orders' }));
    earlier.push(evidence.flush());
    // The twin's thread awaits one last flush when it closes, and is stopped right after: everything must be on disk by then.
    await evidence.flush();
    const stored = JSON.parse(readFileSync(file, 'utf8'));
    assert.deepEqual(Object.keys(stored.segments.all).sort(), ['GET /users/:id', 'POST /orders'], 'both verdicts are on disk');
    await Promise.all(earlier);
    const again = await Evidence.open({ file, candidate: 'c1' });
    assert.equal(again.note, null, 'and the file is whole: its checksum holds');
    assert.equal(again.snapshot().segments.all['POST /orders'].same, 1);
  } finally {
    await Promise.allSettled(earlier);
    rmSync(dir, { recursive: true, force: true });
  }
});

test('each changed line keeps, per character, how many clean comparisons ran it, so the branches of a line add up', () => {
  const evidence = new Evidence({ candidate: 'c1', hashes: new Map([['old:src/app.js:17', 'h17']]) });
  const given = `${'1'.repeat(37)}0000`;
  const blank = `${'1'.repeat(19)}${'0'.repeat(18)}1111`;
  evidence.add(verdict('same', { lines: { 'old:src/app.js:17': given } }));
  evidence.add(verdict('same', { lines: { 'old:src/app.js:17': blank } }));
  evidence.add(verdict('different', { kinds: ['crash'], lines: { 'old:src/app.js:17': blank } }));
  const data = evidence.snapshot();
  assert.deepEqual(data.lines['old:src/app.js:17'], { hash: 'h17', touched: 3, runs: 0, clean: 0, cover: `${'2'.repeat(19)}${'1'.repeat(22)}` }, 'a difference adds nothing to the cover');
  evidence.setLoadLines(['new:src/app.js:1', 'new:src/app.js:1']);
  assert.deepEqual(evidence.snapshot().loadLines, ['new:src/app.js:1']);
});
