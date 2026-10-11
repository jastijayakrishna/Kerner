import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { decodeMappings, lineMasks, mappedMasks, plausible, sourceMapOf, targetsOf } from '../src/coverage.js';

// What V8 reported for this source (Node 22, precise coverage, one take per call), measured on 2026-10-10.
const source = 'export function limitOf(policy) {\n  return policy.limits ? policy.limits.max : 100;\n}\n';
const limitsGiven = [{ functionName: 'limitOf', ranges: [{ startOffset: 7, endOffset: 85, count: 1 }, { startOffset: 77, endOffset: 82, count: 0 }] }];
const limitsBlank = [{ functionName: 'limitOf', ranges: [{ startOffset: 7, endOffset: 85, count: 1 }, { startOffset: 57, endOffset: 76, count: 0 }] }];

test('a span left out may start only where a function or a block can start', () => {
  assert.equal(plausible('  if (admin) wipe();', 13, false), true, 'after a closing parenthesis');
  assert.equal(plausible('  return a ? b + 1 : 0;', 19, false), true, 'at a ternary\'s colon');
  assert.equal(plausible('  return a ? b : c?.d;', 18, false), false, 'an optional chain is no branch');
  assert.equal(plausible('export const x = () => 1;', 0, true), false, 'an export line is a bundler\'s helper');
  assert.equal(plausible('const total = items.map((x) => x * 2);', 24, true), true, 'an arrow function');
  assert.equal(plausible('const total = items.map((x) => x * 2);', 14, true), false, 'the middle of an expression');
});

test('a line\'s mask marks each code character that ran, and one branch of a ternary per request', () => {
  assert.deepEqual(lineMasks(source, limitsGiven, [2]), new Map([[2, `${'1'.repeat(37)}0000`]]), 'the ": 100" branch did not run');
  assert.deepEqual(lineMasks(source, limitsBlank, [2]), new Map([[2, `${'1'.repeat(19)}${'0'.repeat(18)}1111`]]), 'the "? policy.limits.max" branch did not run');
  assert.deepEqual(lineMasks(source, [], [2]), new Map([[2, '0'.repeat(41)]]), 'no span at all: nothing ran');
  assert.deepEqual(lineMasks(source, [{ functionName: '__export', ranges: [{ startOffset: 0, endOffset: 86, count: 1 }] }], [2]), new Map([[2, '0'.repeat(41)]]), 'a bundler helper counts for nothing');
});

test('source maps are decoded and read: a generated line counts for the original line it came from', () => {
  assert.deepEqual(decodeMappings('AAAA;AACA;AACA'), [[[0, 0, 0, 0]], [[0, 0, 1, 0]], [[0, 0, 2, 0]]]);
  assert.deepEqual(decodeMappings('AAAA,EAAE;;C'), [[[0, 0, 0, 0], [2, 0, 0, 2]], [], [[1, -1, -1, -1]]]);
  const generated = 'export function a(x) {\n  return x > 0 ? x : 0;\n}\n';
  const functions = [{ functionName: 'a', ranges: [{ startOffset: 7, endOffset: 48, count: 1 }, { startOffset: 42, endOffset: 45, count: 0 }] }];
  const root = resolve('/srv/app');
  const targets = targetsOf({ side: 'new', root, files: [{ path: 'src/a.ts', lines: [2] }] });
  const masks = mappedMasks(generated, functions, { version: 3, sources: ['../src/a.ts'], mappings: 'AAAA;AACA;AACA' }, join(root, 'dist'), targets);
  assert.deepEqual([...masks.values()], [new Map([[2, `${'1'.repeat(11)}00`]])]);
  assert.deepEqual(masks.get([...targets.keys()][0]).get(2), lineMasks(generated, functions, [2]).get(2), 'the same as reading the file directly');
});

test('a script\'s source map is found inline or in a file beside it', () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-maps-'));
  try {
    const map = { version: 3, sources: ['a.ts'], mappings: 'AAAA' };
    const inline = `x;\n//# sourceMappingURL=data:application/json;base64,${Buffer.from(JSON.stringify(map)).toString('base64')}\n`;
    assert.deepEqual(sourceMapOf(inline, join(dir, 'a.js')), { map, dir });
    writeFileSync(join(dir, 'b.js.map'), JSON.stringify(map));
    assert.deepEqual(sourceMapOf('y;\n//# sourceMappingURL=b.js.map', join(dir, 'b.js')), { map, dir });
    assert.equal(sourceMapOf('z;\n', join(dir, 'c.js')), null);
    assert.equal(sourceMapOf('z;\n//# sourceMappingURL=missing.map', join(dir, 'c.js')), null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
