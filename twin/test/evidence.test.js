import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Evidence } from '../src/evidence.js';
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
