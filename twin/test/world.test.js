import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';

const script = `
import { installWorld, resetWorld } from ${JSON.stringify(new URL('../src/world.js', import.meta.url).href)};
import crypto from 'node:crypto';
installWorld();
const sample = (seed) => {
  resetWorld(1760000000000, seed);
  return [Date.now(), new Date().toISOString(), Date(), Math.random(), crypto.randomUUID(), globalThis.crypto.randomUUID(),
    crypto.randomBytes(4).toString('hex'), performance.now(), new Date(0).getTime(), new Date() instanceof Date];
};
console.log(JSON.stringify([sample(42), sample(42), sample(43)]));
`;

test('a ghost sees a fixed clock and random numbers that repeat for the same seed', () => {
  const [a, b, c] = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8' }));
  assert.deepEqual(a, b);
  assert.equal(a[0], 1760000000000);
  assert.equal(a[1], '2025-10-09T08:53:20.000Z');
  assert.equal(a[7], 0);
  assert.equal(a[8], 0);
  assert.equal(a[9], true);
  assert.notEqual(a[3], c[3]);
  assert.notEqual(a[4], c[4]);
});
