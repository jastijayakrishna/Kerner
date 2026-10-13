import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compareRuns, explain, faithful } from '../src/compare.js';

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
  assert.deepEqual(thrown.details[0], { kind: 'crash', path: 'result', old: 'status 200', new: 'uncaught TypeError: x' });
  assert.equal(compareRuns(run(200, {}), run(0, null, { error: 'no answer within 500 ms', timedOut: true })).kinds[0], 'hang');
});

test('a refused request that now succeeds is a door that opened', () => {
  assert.ok(compareRuns(run(401, { error: 'sign in first' }), run(200, { users: 3 })).kinds.includes('door-opened'));
});

test('a changed field is named by its path', () => {
  const verdict = compareRuns(run(200, { currency: 'JPY', total: 4051 }), run(200, { currency: 'JPY', total: 4050.9 }));
  assert.deepEqual(verdict, { outcome: 'different', kinds: ['body'], details: [{ kind: 'body', path: '$.total', old: '4051', new: '4050.9' }] });
});

test('an extra write is a difference in effects', () => {
  const once = ['POST https://pay.example/charge abc'];
  assert.deepEqual(compareRuns(run(200, {}, { effects: once }), run(200, {}, { effects: [...once, ...once] })).kinds, ['effects']);
});

test('a call a ghost may not make, which both versions make, leaves the request not compared', () => {
  const refused = { events: [{ type: 'denied', detail: 'a network connection to db.example:5432' }] };
  const verdict = compareRuns(run(200, {}, refused), run(200, {}, refused));
  assert.equal(verdict.outcome, 'not-compared');
  assert.deepEqual(verdict.kinds, ['unsupported']);
});

test('the new code reaching for something a ghost may not use, which the old code did not, is a difference; what follows it is not compared', () => {
  // A program started, a connection opened: on the same request, in the same world, only the code changed. Left "not compared", a rare
  // path could switch on with it unseen, under maxUncovered.
  const verdict = compareRuns(run(200, { a: 1 }), run(500, { error: 'x' }, { events: [{ type: 'denied', detail: 'starting the program sh' }] }));
  assert.equal(verdict.outcome, 'different');
  assert.deepEqual(verdict.kinds, ['refused']);
  assert.deepEqual(verdict.details[0], { kind: 'refused', path: 'io', old: 'none', new: 'starting the program sh' });
  assert.match(verdict.details.at(-1).new, /not compared/);
  assert.deepEqual(explain(verdict, '*', [{ route: '*', paths: ['io'] }]).outcome, 'different', 'a path rule never states it');
  assert.deepEqual(explain(verdict, '*', [{ route: '*', kinds: ['refused'] }]).outcome, 'stated');
});

test('a double charge is caught even when the new code then reaches for something a ghost may not use', () => {
  const charge = 'POST https://pay.example/charge abc';
  const verdict = compareRuns(run(200, {}, { effects: [charge] }), run(200, {}, { effects: [charge, charge], events: [{ type: 'denied', detail: 'starting the program sh' }] }));
  assert.equal(verdict.outcome, 'different');
  assert.deepEqual(verdict.kinds, ['effects', 'refused']);
});

test('a call only the old code made that a ghost may not use leaves the request not compared, unless the writes differ', () => {
  // While the new version serves, the old one is the ghost not checked against reality: what it would have answered is unknown.
  const refused = { events: [{ type: 'denied', detail: 'starting the program convert' }] };
  assert.equal(compareRuns(run(200, {}, refused), run(200, {})).outcome, 'not-compared');
  assert.equal(compareRuns(run(200, {}, { ...refused, effects: ['POST https://x.example/ -'] }), run(200, {})).outcome, 'different');
});

test('the old code\'s ghost must reproduce the real status and writes, not the real clock', () => {
  assert.equal(faithful(run(200, { at: 1 }), run(200, { at: 2 })), null);
  assert.match(faithful(run(200, {}), run(503, {})), /answered 503/);
  assert.match(faithful(run(200, {}, { effects: ['POST https://x.example/ -'] }), run(200, {})), /wrote other things/);
});

test('a read the recording cannot answer, made by only one ghost, leaves the request not compared', () => {
  const read = { type: 'unmatched-read', detail: 'GET https://api.example/rates' };
  assert.deepEqual(compareRuns(run(200, { a: 1 }), run(500, { error: 'x' }, { events: [read] })), {
    outcome: 'not-compared',
    kinds: ['unanswered'],
    details: [{ kind: 'unanswered', path: 'calls', old: 'none', new: 'GET https://api.example/rates' }],
  });
});

test('a write one version would make and the other would not is a difference, whatever else happened', () => {
  const charge = 'POST https://pay.example/charge abc';
  const verdict = compareRuns(run(200, {}), run(200, {}, {
    effects: [charge],
    events: [{ type: 'unmatched-write', detail: 'POST https://pay.example/charge' }, { type: 'unmatched-read', detail: 'GET https://pay.example/status' }],
  }));
  assert.equal(verdict.outcome, 'different');
  assert.deepEqual(verdict.kinds, ['effects']);
});

test("after a write the recording cannot answer, a ghost's answer is the replay's, not the version's: the write is the difference", () => {
  // The new version demotes the only admin where the old one refused: its write has no recorded answer, so its ghost then fails.
  const write = 'PG delete from "membership_roles" where "membershipId" = $1';
  const verdict = compareRuns(run(400, { error: 'BadRequest' }), run(500, { error: 'Delete' }, {
    effects: [`${write} 0123456789abcdef 0123456789abcdef`],
    events: [{ type: 'unmatched-write', detail: write }],
  }));
  assert.equal(verdict.outcome, 'different');
  assert.deepEqual(verdict.kinds, ['effects'], 'no crash, status or body: those came from the write the ghost could not make');
  assert.equal(verdict.details[0].path, 'writes');
  assert.match(verdict.details[1].new, /^unknown: the recording cannot answer PG delete from "membership_roles"/);
});

test('a difference an allow rule covers in full is stated; a crash is stated only by naming its kind', () => {
  const renamed = compareRuns(run(200, { user_name: 'ada' }), run(200, { username: 'ada' }));
  assert.equal(explain(renamed, 'GET /user', [{ route: 'GET /user', paths: ['$.user_name', '$.username'], kinds: [] }]).outcome, 'stated');
  assert.equal(explain(renamed, 'GET /user', [{ route: 'GET /other', paths: ['$.user*'], kinds: [] }]).outcome, 'different', 'a rule for another route');
  assert.equal(explain(renamed, 'GET /user', [{ route: '*', paths: ['$.user*'], kinds: [] }]).outcome, 'stated', 'a prefix rule for every route');
  assert.equal(explain(renamed, 'GET /user', [{ route: '*', paths: ['$.user_name'], kinds: [] }]).outcome, 'different', 'one detail left uncovered');
  const crash = compareRuns(run(200, {}), run(500, {}));
  assert.equal(explain(crash, 'GET /user', [{ route: '*', paths: ['status', '$*'], kinds: [] }]).outcome, 'different', 'a path rule never states a crash');
  assert.equal(explain(crash, 'GET /user', [{ route: '*', paths: [], kinds: ['crash'] }]).outcome, 'stated');
  assert.equal(explain(compareRuns(run(200, {}), run(200, {})), 'GET /user', [{ route: '*', paths: [], kinds: ['crash'] }]).outcome, 'same');
});

test('the version that served is the one checked against what really happened', () => {
  assert.match(faithful(run(200, {}), run(503, {}), 'candidate'), /new code's ghost answered 503, the real run 200/);
  assert.match(faithful(run(200, {}), run(503, {})), /old code's ghost answered 503/);
});

test('the serving ghost must make writes of the same shapes as the real run; file writes are compared between the ghosts only', () => {
  const real = run(200, {}, { effects: ['POST https://x.example/events aaaa'] });
  assert.equal(faithful(real, run(200, {}, { effects: ['POST https://x.example/events bbbb', 'FS writeFile /tmp/x -'] })), null);
  assert.match(faithful(real, run(200, {}, { effects: ['DELETE https://x.example/events bbbb'] })), /wrote other things/);
});
