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

test("a version's name covers the queries, templates and schemas its code reads, and never the files it writes as it runs", async () => {
  const root = mkdtempSync(join(tmpdir(), 'kern-identity-'));
  try {
    writeFileSync(join(root, 'package.json'), '{"type":"module"}');
    for (const dir of ['src', 'db', 'views', 'logs', 'uploads']) mkdirSync(join(root, dir));
    const entry = join(root, 'src', 'app.js');
    writeFileSync(entry, 'export const createApp = () => (req, res) => res.end("a");\n');
    writeFileSync(join(root, 'db', 'orders.sql'), 'select * from orders where id = $1;\n');
    writeFileSync(join(root, 'views', 'order.ejs'), '<p><%= order.total %></p>\n');
    const first = await versionId(entry, 'createApp');
    writeFileSync(join(root, 'db', 'orders.sql'), 'select * from orders where id = $1 and deleted_at is null;\n');
    const second = await versionId(entry, 'createApp');
    assert.notEqual(second, first, 'a changed query is a changed version');
    writeFileSync(join(root, 'views', 'order.ejs'), '<p><%= order.total.toFixed(2) %></p>\n');
    const third = await versionId(entry, 'createApp');
    assert.notEqual(third, second, 'so is a changed template');
    writeFileSync(join(root, 'logs', 'app.log'), 'GET /orders/1 200\n');
    writeFileSync(join(root, 'uploads', 'receipt.png'), 'not really a picture');
    assert.equal(await versionId(entry, 'createApp'), third, 'a log or an upload the app writes as it runs is not code');
    const read = ['schema.graphql', 'schema.gql', 'schema.prisma', 'api.proto', 'config.yaml', 'config.yml', 'config.toml', 'page.hbs', 'page.handlebars',
      'page.pug', 'page.mustache', 'page.njk', 'page.html', 'App.vue', 'App.svelte', 'Page.astro', 'addon.node', 'module.wasm', 'OLD.SQL'];
    for (const name of read) {
      writeFileSync(join(root, 'src', name), 'one');
      const before = await versionId(entry, 'createApp');
      writeFileSync(join(root, 'src', name), 'two');
      assert.notEqual(await versionId(entry, 'createApp'), before, `${name} is part of the version`);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
