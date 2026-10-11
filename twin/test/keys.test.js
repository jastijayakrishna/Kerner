import { test } from 'node:test';
import assert from 'node:assert/strict';
import { callKey, pgIsWrite, pgKey, readAllowed, redisIsWrite, redisKey, shapeOf, stable } from '../src/keys.js';

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
