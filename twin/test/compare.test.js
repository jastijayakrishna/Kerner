import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compareRuns, explain, faithful } from '../src/compare.js';
import { digest } from '../src/keys.js';

const run = (status, value, more = {}) => ({
  status,
  headers: { 'content-type': 'application/json', date: more.date ?? 'Mon, 01 Jan 2024 12:00:00 GMT' },
  body: Buffer.from(JSON.stringify(value)),
  effects: more.effects ?? [],
  events: more.events ?? [],
  error: more.error,
  timedOut: more.timedOut,
  after: more.after,
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

test('two ghosts that both ran out of time are not compared: neither answered, so nothing is known of either', () => {
  const hung = run(0, null, { error: 'no answer within 500 ms', timedOut: true });
  assert.deepEqual(compareRuns(hung, hung), {
    outcome: 'not-compared',
    kinds: ['no-answer'],
    details: [{ kind: 'no-answer', path: 'result', old: 'no answer within 500 ms', new: 'no answer within 500 ms' }],
  });
  const charge = 'POST https://pay.example/charge abc';
  assert.deepEqual(compareRuns(hung, { ...hung, effects: [charge] }).kinds, ['effects'], 'a write only one of them made still differs');
  // A ghost that had stopped before the request came never saw it.
  const gone = { ...run(0, null, { error: 'the ghost had stopped before this request' }), exited: true, gone: true };
  assert.equal(compareRuns(gone, gone).outcome, 'not-compared');
  assert.deepEqual(compareRuns(hung, gone).kinds, ['no-answer']);
  // Two versions that fail the same way on the same request behave alike: that was compared, and it is the same.
  const crashed = run(0, null, { error: 'uncaught TypeError: x' });
  assert.equal(compareRuns(crashed, crashed).outcome, 'same');
});

test('a body over the cap is compared by its size and its sha256: against a smaller body it differs, and two alike are the same', () => {
  // A ghost hands over at most maxBodyBytes of its answer; past it, only `large`: the size and the sha256 of all of it.
  const big = (size, hash) => ({ ...run(200, null), body: Buffer.alloc(0), large: { size, digest: hash } });
  const small = run(200, { count: 70000 });
  const [x, y] = ['a'.repeat(64), 'b'.repeat(64)];
  assert.deepEqual(compareRuns(small, big(2097152, x)), {
    outcome: 'different',
    kinds: ['body'],
    details: [{ kind: 'body', path: 'body', old: `${small.body.length} bytes, sha256 ${digest(small.body).slice(0, 16)}`, new: `2097152 bytes, sha256 ${'a'.repeat(16)}` }],
  });
  assert.equal(compareRuns(big(2097152, x), big(2097152, x)).outcome, 'same');
  assert.deepEqual(compareRuns(big(2097152, x), big(2097152, y)).kinds, ['body']);
  // A ghost that kept the whole body (a case replayed alone, with no limit) is measured the same way.
  const whole = Buffer.from('z'.repeat(2048));
  assert.equal(compareRuns({ ...run(200, null), body: whole }, big(whole.length, digest(whole))).outcome, 'same');
});

test('a refused request that now succeeds is a door that opened', () => {
  assert.ok(compareRuns(run(401, { error: 'sign in first' }), run(200, { users: 3 })).kinds.includes('door-opened'));
});

test('a changed field is named by its path', () => {
  const verdict = compareRuns(run(200, { currency: 'JPY', total: 4051 }), run(200, { currency: 'JPY', total: 4050.9 }));
  assert.deepEqual(verdict, { outcome: 'different', kinds: ['body'], details: [{ kind: 'body', path: '$.total', old: '4051', new: '4050.9' }] });
});

test('every write a ghost made is compared, by key and how often, not only the first 200 it lists', () => {
  // A ghost lists its first 200 writes in `effects`, and counts every one of them in `writes`.
  const row = 'POST https://rows.example/ 0123456789abcdef';
  const many = (n) => ({ ...run(200, {}), effects: Array(Math.min(n, 200)).fill(row), writes: [[row, n]] });
  const verdict = compareRuns(many(300), many(301));
  assert.deepEqual(verdict.kinds, ['effects']);
  assert.deepEqual(verdict.details[0], { kind: 'effects', path: 'writes', old: `${row} (300 times)`, new: `${row} (301 times)` });
  assert.equal(compareRuns(many(300), many(300)).outcome, 'same');
  // The real run's record lists every write: the serving ghost must have made all of them, by shape.
  const real = run(200, {}, { effects: Array.from({ length: 300 }, (_, i) => `POST https://rows.example/ ${String(i).padStart(16, '0')}`) });
  assert.equal(faithful(real, many(300)), null);
  assert.match(faithful(real, many(299)), /wrote other things/);
});

test('a ghost that made more different writes than it counts is never called the same', () => {
  const over = { ...run(200, {}), effects: [], writes: [], tooMany: true };
  assert.deepEqual(compareRuns(over, over), {
    outcome: 'not-compared',
    kinds: ['too-many-writes'],
    details: [{ kind: 'too-many-writes', path: 'writes', old: 'more different writes than a ghost counts', new: 'more different writes than a ghost counts' }],
  });
  assert.equal(compareRuns(over, { ...over, status: 500 }).outcome, 'different', 'what else differs is still a difference');
  assert.deepEqual(compareRuns(run(200, {}), over).kinds, ['effects'], 'only one of them made that many: their writes differ');
  assert.match(faithful(run(200, {}), over), /more different writes than a ghost counts/);
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

test('a version that fails after it has answered is a crash, though its answer was the same; both failing alike behave alike', () => {
  // An uncaught error in work the request left running ends a production process that has no handler for it.
  const failed = { after: 'uncaught Error: the audit entry could not be written' };
  const verdict = compareRuns(run(200, { ok: true }), run(200, { ok: true }, failed));
  assert.equal(verdict.outcome, 'different');
  assert.deepEqual(verdict.kinds, ['crash']);
  assert.deepEqual(verdict.details[0], { kind: 'crash', path: 'after the answer', old: 'none', new: failed.after });
  assert.equal(compareRuns(run(200, {}, failed), run(200, {}, failed)).outcome, 'same');
});

test('a request the old version failed on and the new version now hangs on is a hang, which a path rule never states', () => {
  const verdict = compareRuns(run(0, null, { error: 'uncaught TypeError: x' }), run(0, null, { error: 'no answer within 500 ms', timedOut: true }));
  assert.deepEqual(verdict.kinds, ['hang']);
  assert.equal(explain(verdict, '*', [{ route: '*', paths: ['result'] }]).outcome, 'different');
});

test("a Set-Cookie is compared by its name, its attributes and a plain value; a session token's own value is not", () => {
  const cookies = (...list) => ({ status: 200, headers: { 'content-type': 'application/json', 'set-cookie': list }, body: Buffer.from('{}'), effects: [], events: [] });
  const session = (token, flags = 'Path=/; HttpOnly; Secure; SameSite=Lax') => `sid=${token}; ${flags}`;
  // A session id drawn after one more random number differs, though nothing that matters did.
  assert.equal(compareRuns(cookies(session('s%3A9f8e7d6c5b4a3f2e1d0c'), 'lang=en'), cookies(session('s%3A1a2b3c4d5e6f7a8b9c0d'), 'lang=en')).outcome, 'same');
  assert.equal(compareRuns(cookies(session('s%3A9f8e7d6c5b4a3f2e1d0c')), cookies('sid=s%3A9f8e7d6c5b4a3f2e1d0c; secure; httponly; samesite=Lax; path=/')).outcome, 'same', 'attributes in any order and case');
  const weaker = compareRuns(cookies(session('s%3A9f8e7d6c5b4a3f2e1d0c')), cookies(session('s%3A9f8e7d6c5b4a3f2e1d0c', 'Path=/; SameSite=Lax')));
  assert.deepEqual([weaker.outcome, weaker.kinds, weaker.details[0].path], ['different', ['headers'], 'header set-cookie'], 'HttpOnly and Secure dropped');
  assert.equal(compareRuns(cookies(session('s%3A9f8e7d6c5b4a3f2e1d0c'), 'lang=en'), cookies(session('s%3A9f8e7d6c5b4a3f2e1d0c'))).outcome, 'different', 'a cookie no longer set');
  assert.equal(compareRuns(cookies('role=user; Path=/'), cookies('role=admin; Path=/')).outcome, 'different', 'a plain value that changed');
});
