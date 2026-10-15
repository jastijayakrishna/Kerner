import { test } from 'node:test';
import assert from 'node:assert/strict';
import { callKey, digest, isSession, pgIsWrite, pgKey, readAllowed, redisIsWrite, redisKey, shapeOf, stable } from '../src/keys.js';

test('every call is named <shape> <payload>; the shape is all but the last word', () => {
  assert.equal(shapeOf(callKey('post', 'https://pay.example/charge', Buffer.from('{"a":1}'))), 'POST https://pay.example/charge');
  assert.equal(callKey('GET', 'https://x.example/', null), 'GET https://x.example/ -');
  const insert = pgKey('INSERT INTO orders (total)\n  VALUES ($1)', [12.5]);
  assert.match(insert, /^PG INSERT INTO orders \(total\) VALUES \(\$1\) [0-9a-f]{16} [0-9a-f]{16}$/);
  assert.equal(shapeOf(insert), shapeOf(pgKey('INSERT INTO orders (total) VALUES ($1)', [99])), 'other values, the same shape');
  assert.notEqual(insert, pgKey('INSERT INTO orders (total) VALUES ($1)', [99]));
  assert.match(pgKey('BEGIN', []), /^PG BEGIN [0-9a-f]{16} -$/);
  assert.equal(shapeOf(redisKey('set', ['session:1', 'a'])), shapeOf(redisKey('SET', ['session:1', 'b'])));
  assert.notEqual(shapeOf(redisKey('set', ['session:1', 'a'])), shapeOf(redisKey('set', ['session:2', 'a'])), 'another Redis key is another shape');
});

test("a savepoint's name is a label the client library numbers itself: it does not change the call's shape", () => {
  const shape = (text) => shapeOf(pgKey(text, []));
  assert.equal(shape('SAVEPOINT trx9;'), shape('SAVEPOINT trx153;'));
  assert.equal(shape('RELEASE SAVEPOINT trx9;'), shape('release savepoint "trx153"'));
  assert.equal(shape('ROLLBACK TO SAVEPOINT trx9;'), shape('ROLLBACK TO trx153;'));
  assert.notEqual(shape('SAVEPOINT trx9;'), shape('RELEASE SAVEPOINT trx9;'), 'setting, releasing and rolling back stay apart');
  assert.notEqual(shape('SELECT 1'), shape('SELECT 2'), 'other queries keep their text');
});

test('transaction control is a session call: a new code path may open a transaction the recording never began', () => {
  for (const query of ['BEGIN', 'begin;', 'BEGIN TRANSACTION', 'BEGIN ISOLATION LEVEL SERIALIZABLE', 'START TRANSACTION', 'COMMIT', 'commit;', 'END', 'ROLLBACK', 'ABORT']) {
    assert.ok(isSession(pgKey(query, [])), query);
  }
  assert.ok(isSession(pgKey('select version()', [])) && isSession(pgKey('SET search_path = x', [])));
  for (const query of ['SELECT * FROM users WHERE id = $1', 'INSERT INTO t VALUES (1)', 'ROLLBACK TO SAVEPOINT trx9']) {
    assert.ok(!isSession(pgKey(query, [1])), query);
  }
});

test('which queries and commands write', () => {
  for (const query of ['INSERT INTO t VALUES (1)', '  update t set a = 1', 'WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x', "SELECT nextval('s')", '/* note */ TRUNCATE t']) assert.equal(pgIsWrite(query), true, query);
  for (const query of ['SELECT * FROM t', 'BEGIN', 'COMMIT', 'WITH x AS (SELECT 1) SELECT * FROM x', 'SHOW search_path']) assert.equal(pgIsWrite(query), false, query);
  assert.deepEqual(['GET', 'hgetall', 'SET', 'INCR', 'EVAL', 'XADD'].map(redisIsWrite), [false, false, true, true, true, true]);
});

test('values are named alike in every thread, and real reads are matched by method and address', () => {
  const value = [Buffer.from('ab'), new Date(0), 10n, { a: 1 }];
  assert.equal(stable(value), stable(structuredClone(value)), 'a Buffer arrives in another thread as a Uint8Array');
  const allowed = ['GET https://rates.example/*', 'HEAD https://status.example/ping'];
  assert.equal(readAllowed(allowed, 'GET', 'https://rates.example/eur?x=1'), true);
  assert.equal(readAllowed(allowed, 'POST', 'https://rates.example/eur'), false, 'never a write');
  assert.equal(readAllowed(allowed, 'GET', 'https://status.example/ping'), false, 'the method must match');
  assert.equal(readAllowed(allowed, 'HEAD', 'https://status.example/ping'), true);
});

test("a JSON payload is named by its value: the order of an object's keys and the spacing do not change a call's name", () => {
  const url = 'https://pay.example/charge';
  const a = callKey('POST', url, Buffer.from('{"amount":5,"currency":"usd","meta":{"b":1,"a":2}}'));
  assert.equal(a, callKey('POST', url, Buffer.from('{ "currency": "usd", "meta": {"a": 2, "b": 1}, "amount": 5 }')));
  assert.notEqual(a, callKey('POST', url, Buffer.from('{"amount":6,"currency":"usd","meta":{"b":1,"a":2}}')), 'another value is another call');
  assert.notEqual(callKey('POST', url, Buffer.from('[1,2]')), callKey('POST', url, Buffer.from('[2,1]')), 'an array keeps its order');
  assert.notEqual(callKey('POST', url, Buffer.from('5')), callKey('POST', url, Buffer.from(' 5')), 'a body that is not a JSON object or array is named by its bytes');
  assert.equal(callKey('POST', url, Buffer.from('{"a":1}')), `POST ${url} ${digest('{"a":1}').slice(0, 16)}`, 'a body already in order keeps its name');
  const deep = `${'['.repeat(100000)}{"b":1,"a":2}${']'.repeat(100000)}`;
  assert.equal(callKey('POST', url, Buffer.from(deep)), callKey('POST', url, Buffer.from(deep.replace('{"b":1,"a":2}', '{"a":2,"b":1}'))), 'at any depth');
});

test('a real read goes only to the origin its pattern names, and a path that climbs out of the prefix is not under it', () => {
  const allowed = ['GET https://rates.example/eur/*', 'GET https://api.example.com*'];
  assert.equal(readAllowed(allowed, 'GET', 'https://rates.example/eur/today'), true);
  assert.equal(readAllowed(allowed, 'GET', 'https://rates.example/eur/../admin'), false, 'dot segments are resolved, as fetch resolves them');
  assert.equal(readAllowed(allowed, 'GET', 'https://rates.example/eur/%2e%2e/admin'), false, 'encoded dot segments too');
  assert.equal(readAllowed(allowed, 'GET', 'https://api.example.com/v1/x'), true);
  assert.equal(readAllowed(allowed, 'GET', 'https://api.example.com.other.test/v1'), false, 'another host that starts with the same letters');
  assert.equal(readAllowed(allowed, 'GET', 'https://api.example.com@other.test/v1'), false, 'a user name that looks like the host');
  assert.equal(readAllowed(allowed, 'GET', 'https://api.example.com:8443/v1'), false, 'another port is another origin');
  assert.equal(readAllowed(allowed, 'GET', 'not an address'), false);
});
