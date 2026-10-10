import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compareRuns, faithful } from '../src/compare.js';

const run = (status, value, more = {}) => ({
  status,
  headers: { 'content-type': 'application/json', date: more.date ?? 'Mon, 01 Jan 2024 12:00:00 GMT' },
  body: Buffer.from(JSON.stringify(value)),
  effects: more.effects ?? [],
  events: more.events ?? [],
  error: more.error,
  timedOut: more.timedOut,
});

test('the same answer is the same, whatever the date header says', () => {
  assert.deepEqual(compareRuns(run(200, { a: 1 }), run(200, { a: 1 }, { date: 'Tue, 02 Jan 2024 12:00:00 GMT' })), { outcome: 'same', kinds: [], details: [] });
});

test('a server error or a thrown error where the old code answered is a crash; no answer at all is a hang', () => {
  assert.ok(compareRuns(run(200, { ok: true }), run(500, { error: 'x' })).kinds.includes('crash'));
  const thrown = compareRuns(run(200, {}), run(0, null, { error: 'uncaught TypeError: x' }));
  assert.equal(thrown.kinds[0], 'crash');
  assert.deepEqual(thrown.details[0], { path: 'result', old: 'status 200', new: 'uncaught TypeError: x' });
  assert.equal(compareRuns(run(200, {}), run(0, null, { error: 'no answer within 500 ms', timedOut: true })).kinds[0], 'hang');
});

test('a refused request that now succeeds is a door that opened', () => {
  assert.ok(compareRuns(run(401, { error: 'sign in first' }), run(200, { users: 3 })).kinds.includes('door-opened'));
});

test('a changed field is named by its path', () => {
  const verdict = compareRuns(run(200, { currency: 'JPY', total: 4051 }), run(200, { currency: 'JPY', total: 4050.9 }));
  assert.deepEqual(verdict, { outcome: 'different', kinds: ['body'], details: [{ path: '$.total', old: '4051', new: '4050.9' }] });
});

test('an extra write is a difference in effects', () => {
  const once = ['POST https://pay.example/charge abc'];
  assert.deepEqual(compareRuns(run(200, {}, { effects: once }), run(200, {}, { effects: [...once, ...once] })).kinds, ['effects']);
});

test('a call a ghost may not make leaves the request not compared', () => {
  const verdict = compareRuns(run(200, {}), run(200, {}, { events: [{ type: 'denied', detail: 'a network connection to db.example:5432' }] }));
  assert.equal(verdict.outcome, 'not-compared');
});

test('the old code\'s ghost must reproduce the real status and writes, not the real clock', () => {
  assert.equal(faithful(run(200, { at: 1 }), run(200, { at: 2 })), null);
  assert.match(faithful(run(200, {}), run(503, {})), /answered 503/);
  assert.match(faithful(run(200, {}, { effects: ['POST https://x.example/ -'] }), run(200, {})), /wrote other things/);
});
