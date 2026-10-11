import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Evidence } from '../src/evidence.js';
import { decide, needed, pathsOf, totals } from '../src/decide.js';
import { acceptMatcher } from '../src/manifest.js';
import { T, verdict } from './verdicts.js';

test('a candidate is ready exactly when it has ceil(3/ε) clean comparisons', () => {
  assert.deepEqual([needed(0.1), needed(0.01), needed(0.003), needed(0.001)], [30, 300, 1000, 3000]);
  const policy = { epsilon: 0.01, minHours: 0, segments: ['*'], maxUncovered: 0.05 };
  const evidence = new Evidence({ candidate: 'c1' });
  for (let i = 0; i < 299; i++) evidence.add(verdict(i % 10 === 0 ? 'stated' : 'same'));
  const before = decide(evidence.snapshot(), policy, Date.now());
  assert.equal(before.ready, false);
  assert.deepEqual(before.reasons, ['all traffic: 299 of 300 clean comparisons']);
  evidence.add(verdict('same'));
  const after = decide(evidence.snapshot(), policy, Date.now());
  assert.equal(after.ready, true, after.reasons.join('; '));
  assert.equal(after.bound['*'], 0.01, 'the risk bound: 3 / 300');
  assert.deepEqual(totals(evidence.snapshot(), '*'), { same: 270, stated: 30, different: 0, notCompared: 0, skipped: 0 });
});

test('an unexplained difference, too much left uncompared, a thin segment or too little time each hold a candidate back', () => {
  const policy = { epsilon: 0.1, minHours: 24, segments: ['pro', 'free'], maxUncovered: 0.05 };
  const evidence = new Evidence({ candidate: 'c1' });
  for (let i = 0; i < 30; i++) evidence.add(verdict('same', { segment: 'pro', time: T }));
  for (let i = 0; i < 2; i++) evidence.add(verdict('not-compared', { segment: 'pro', time: T }));
  for (let i = 0; i < 10; i++) evidence.add(verdict('same', { segment: 'free', time: T }));
  evidence.add(verdict('different', { segment: 'free', time: T, kinds: ['body'] }));
  const { ready, reasons } = decide(evidence.snapshot(), policy, T + 3600000);
  assert.equal(ready, false);
  assert.deepEqual(reasons, [
    '1 unexplained difference',
    'segment pro: 6.3% of requests could not be compared (at most 5.0% may be)',
    'segment free: 10 of 30 clean comparisons',
    '1.0 of 24 hours watched',
  ]);
});

test('every changed line must have run, every part of it, in minLineRuns clean comparisons, unless it ran at load or a person accepted it', () => {
  const evidence = new Evidence({ candidate: 'c1' });
  for (let i = 0; i < 6; i++) evidence.add(verdict('same', { lines: { 'old:src/app.js:17': i % 2 ? '1100' : '1111', 'new:src/app.js:17': '1111' } }));
  evidence.setLoadLines(['new:src/app.js:13']);
  const policy = {
    epsilon: 0.5, minHours: 0, segments: ['*'], maxUncovered: 0.05, minLineRuns: 3, accepted: acceptMatcher(['new:src/app.js:9']),
    lineKeys: ['old:src/app.js:17', 'new:src/app.js:17', 'new:src/app.js:9', 'new:src/app.js:13', 'old:src/app.js:9'],
  };
  const held = decide(evidence.snapshot(), policy, Date.now());
  assert.deepEqual(held.waiting, ['old:src/app.js:9']);
  assert.deepEqual(held.reasons, ['1 changed line has not yet run in full in 3 clean comparisons: old:src/app.js:9']);
  const ready = decide(evidence.snapshot(), { ...policy, lineKeys: policy.lineKeys.slice(0, 4) }, Date.now());
  assert.equal(ready.ready, true, ready.reasons.join('; '));
  const stricter = decide(evidence.snapshot(), { ...policy, minLineRuns: 4, lineKeys: ['old:src/app.js:17'] }, Date.now());
  assert.deepEqual(stricter.waiting, ['old:src/app.js:17'], 'its last two characters ran in only 3 clean comparisons');
  const lines = new Map([
    ['old:src/app.js:17', { side: 'old', path: 'src/app.js', line: 17, text: 'a', pull: 103 }],
    ['old:src/app.js:9', { side: 'old', path: 'src/app.js', line: 9, text: 'b', pull: 101 }],
  ]);
  assert.deepEqual(pathsOf(evidence.snapshot(), lines, policy).map((p) => [p.key, p.status, p.clean]), [['old:src/app.js:17', 'proven', 3], ['old:src/app.js:9', 'never run', 0]]);
});
