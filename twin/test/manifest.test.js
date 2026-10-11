import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { acceptMatcher, attribute, checkRoots, coveragePlan, loadManifest } from '../src/manifest.js';

const fixture = (path) => fileURLToPath(new URL(`./apps/three-prs/${path}`, import.meta.url));

test('a manifest gives every changed line a key, its text, a hash and its pull request', async () => {
  const manifest = await loadManifest(fixture('manifest.json'));
  assert.deepEqual([...manifest.lines.keys()], ['old:src/app.js:9', 'old:src/app.js:13', 'old:src/app.js:17', 'new:src/app.js:9', 'new:src/app.js:13', 'new:src/app.js:17']);
  const line = manifest.lines.get('new:src/app.js:17');
  assert.deepEqual([line.side, line.path, line.line, line.text, line.pull], ['new', 'src/app.js', 17, '  return policy.limits.max;', 103]);
  assert.match(line.hash, /^[0-9a-f]{16}$/);
  assert.deepEqual(coveragePlan(manifest, 'new', '/srv/next'), { side: 'new', root: '/srv/next', files: [{ path: 'src/app.js', lines: [9, 13, 17] }] });
  await assert.rejects(() => loadManifest(fixture('base/src/app.js')), /is not JSON/);
});

test('the code under each root must be the code the manifest describes', async () => {
  const manifest = await loadManifest(fixture('manifest.json'));
  await checkRoots(manifest, { old: fixture('base'), new: fixture('head') });
  await assert.rejects(() => checkRoots(manifest, { old: fixture('head'), new: fixture('head') }), /the old code at .* is not what the manifest describes: src\/app\.js:9/);
  await assert.rejects(() => checkRoots(manifest, { old: fixture('base'), new: fixture('nowhere') }), /the new code has no file src\/app\.js/);
});

test('a difference names the one pull request whose lines it ran, and accepted lines are matched by side, path and line', async () => {
  const manifest = await loadManifest(fixture('manifest.json'));
  assert.deepEqual(attribute(['old:src/app.js:17', 'new:src/app.js:17'], manifest), { pulls: [103], cause: 103 });
  assert.deepEqual(attribute(['new:src/app.js:9', 'new:src/app.js:13'], manifest), { pulls: [101, 102], cause: null });
  assert.deepEqual(attribute([], manifest), { pulls: [], cause: null });
  const accepted = acceptMatcher(['old:src/app.js:17', 'new:src/app.js:9-13']);
  assert.deepEqual(['old:src/app.js:17', 'new:src/app.js:13', 'new:src/app.js:17', 'old:src/app.js:9'].map(accepted), [true, true, false, false]);
  assert.throws(() => acceptMatcher(['src/app.js:17']), /accept entries look like/);
});
