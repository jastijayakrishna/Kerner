import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Evidence } from '../src/evidence.js';
import { decide, needed, totals } from '../src/decide.js';
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
