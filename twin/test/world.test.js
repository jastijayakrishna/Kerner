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

test('every way Node fills bytes at random repeats for the same seed, the way id generators such as nanoid draw them', () => {
  const [a, b, c] = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installWorld, resetWorld } from ${JSON.stringify(new URL('../src/world.js', import.meta.url).href)};
import crypto, { randomFillSync as named } from 'node:crypto';
installWorld();
const sample = async (seed) => {
  resetWorld(1760000000000, seed);
  const filled = crypto.randomFillSync(Buffer.alloc(8)).toString('hex');
  const part = crypto.randomFillSync(Buffer.alloc(8), 4, 2).toString('hex');
  const imported = named(new Uint8Array(4)).join(',');
  const array = Buffer.from(crypto.randomFillSync(new ArrayBuffer(4))).toString('hex');
  const web = Buffer.from(crypto.webcrypto.getRandomValues(new Uint8Array(4))).toString('hex');
  const later = await new Promise((resolve) => crypto.randomFill(Buffer.alloc(4), (error, out) => resolve(out.toString('hex'))));
  return [filled, part, imported, array, web, later];
};
console.log(JSON.stringify([await sample(42), await sample(42), await sample(43)]));
`], { encoding: 'utf8' }));
  assert.deepEqual(a, b, 'the same seed, the same bytes');
  assert.match(a[1], /^00000000[0-9a-f]{4}0000$/, 'only the asked-for range is filled');
  assert.notDeepEqual(a, c);
});

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
