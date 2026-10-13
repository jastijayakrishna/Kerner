import { test } from 'node:test';
import assert from 'node:assert/strict';
import { gzipSync } from 'node:zlib';
import { diffValues, readBody, preview } from '../src/serialize.js';
import { callKey, isWrite, digest } from '../src/keys.js';

test('differences are named by their path', () => {
  const found = diffValues({ total: 4051, items: [1, 2], name: 'a' }, { total: 4050.9, items: [1, 3], extra: true, name: 'a' });
  assert.deepEqual(found, [
    { path: '$.extra', old: 'missing', new: 'true' },
    { path: '$.items[1]', old: '2', new: '3' },
    { path: '$.total', old: '4051', new: '4050.9' },
  ]);
});

test('equal values have no differences, and names from the prototype do not count as present', () => {
  assert.deepEqual(diffValues({ a: [1, { b: null }] }, { a: [1, { b: null }] }), []);
  assert.deepEqual(diffValues({}, { constructor: 1 }), [{ path: '$.constructor', old: 'missing', new: '1' }]);
});

test('a compressed JSON body is read by value, text as text, anything else by its hash', () => {
  const body = gzipSync(Buffer.from(JSON.stringify({ ok: true })));
  assert.deepEqual(readBody(body, { 'content-type': 'application/json', 'content-encoding': 'gzip' }), { kind: 'json', value: { ok: true } });
  assert.equal(readBody(Buffer.from('<p>hi</p>'), { 'content-type': 'text/html' }).kind, 'text');
  assert.equal(readBody(Buffer.from([1, 2, 3]), {}).kind, 'bytes');
});

test('long values are cut short for showing', () => {
  assert.equal(preview('x'.repeat(500)).length, 120);
});

const nested = (depth, leaf) => {
  let value = leaf;
  for (let i = 0; i < depth; i++) value = [value];
  return value;
};

test('values nested a hundred thousand deep are compared without running out of stack', () => {
  // JSON.parse reads any depth; a body that deep must not end the twin when it is compared.
  assert.deepEqual(diffValues(nested(100000, 1), nested(100000, 1)), []);
  const found = diffValues(nested(100000, 1), nested(100000, 2));
  assert.equal(found.length, 1);
  assert.deepEqual([found[0].old, found[0].new], ['1', '2']);
  assert.ok(found[0].path.length <= 200, `a path that deep is shown cut short (${found[0].path.length})`);
  assert.match(found[0].path, /^\$\[0\]\[0\]\[0\].*\.\.\..*\[0\]\[0\]$/);
});

test('the order of differences stays the order of the values, however they are walked', () => {
  const found = diffValues({ a: [1, { b: 2, c: 3 }], d: 4, z: { y: 1 } }, { a: [1, { b: 5 }, 9], e: 6, z: { y: 2 } });
  assert.deepEqual(found.map((d) => d.path), ['$.a.length', '$.a[1].b', '$.a[1].c', '$.d', '$.e', '$.z.y']);
});

test('a value nested too deeply to write out is shown by its kind', () => {
  assert.equal(preview(nested(100000, 1)), 'an array of 1, nested too deeply to show');
  assert.equal(preview({ a: nested(100000, 1), b: 2 }), 'an object of 2 keys, nested too deeply to show');
});

test('a call has the same name everywhere, and only reads are not writes', () => {
  assert.equal(callKey('post', 'https://pay.example/charge', Buffer.from('{"amount":5}')), `POST https://pay.example/charge ${digest('{"amount":5}').slice(0, 16)}`);
  assert.equal(callKey('GET', 'https://api.example/x', Buffer.alloc(0)), 'GET https://api.example/x -');
  assert.equal(isWrite('GET'), false);
  assert.equal(isWrite('delete'), true);
});
