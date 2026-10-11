import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { versionId } from '../src/identity.js';

test('a version is named by its code: any code change gives a new name, other files do not', async () => {
  const root = mkdtempSync(join(tmpdir(), 'kern-identity-'));
  try {
    writeFileSync(join(root, 'package.json'), '{"type":"module"}');
    mkdirSync(join(root, 'src'));
    const entry = join(root, 'src', 'app.js');
    writeFileSync(entry, 'export const createApp = () => (req, res) => res.end("a");\n');
    const first = await versionId(entry, 'createApp');
    assert.match(first, /^sha256:[0-9a-f]{64}$/);
    writeFileSync(join(root, 'README.md'), 'notes');
    assert.equal(await versionId(entry, 'createApp'), first, 'a note is not code');
    mkdirSync(join(root, 'node_modules', 'dep'), { recursive: true });
    writeFileSync(join(root, 'node_modules', 'dep', 'index.js'), 'module.exports = 1;');
    assert.equal(await versionId(entry, 'createApp'), first, 'installed packages are named by the lockfile, not walked');
    writeFileSync(entry, 'export const createApp = () => (req, res) => res.end("b");\n');
    const second = await versionId(entry, 'createApp');
    assert.notEqual(second, first);
    assert.notEqual(await versionId(entry, 'otherApp'), second, 'another entry point is another version');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
