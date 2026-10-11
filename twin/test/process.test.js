import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { listen, post, production } from './helpers.js';

test('process ghosts find what thread ghosts find, and charge no one', async () => {
  const renamed = await production('renamed-field', { ghosts: 'process' });
  assert.equal(renamed.twin.stats().disabled, null, renamed.twin.stats().disabled);
  await (await fetch(`${renamed.url}/user`)).arrayBuffer();
  await renamed.twin.idle();
  assert.ok(renamed.twin.differences()[0].details.some((d) => d.path === '$.username'), JSON.stringify(renamed.twin.differences()));
  await renamed.close();
  let charges = 0;
  const pay = await listen((req, res) => {
    charges++;
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true}');
    });
  });
  process.env.KERN_TEST_PAY_URL = pay.url;
  try {
    const shop = await production('double-charge', { ghosts: 'process' });
    assert.deepEqual(await (await post(shop.url, { amount: 25 })).json(), { paid: true });
    await shop.twin.idle();
    assert.equal(charges, 1);
    assert.ok(shop.twin.differences()[0].kinds.includes('effects'));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_PAY_URL;
    await pay.close();
  }
});

test('code loaded before the seal escapes a thread ghost, but not Node\'s permission model in a process ghost', async () => {
  const keep = new URL('./apps/escape/keep.mjs', import.meta.url).href;
  const wrote = {};
  for (const ghosts of ['thread', 'process']) {
    const file = join(tmpdir(), `kern-escape-${process.pid}-${ghosts}.txt`);
    process.env.KERN_TEST_ESCAPE_FILE = file;
    try {
      const shop = await production('escape', { ghosts, ghostExecArgv: ['--import', keep] });
      assert.equal(shop.twin.stats().disabled, null, shop.twin.stats().disabled);
      await (await fetch(shop.url)).arrayBuffer();
      await shop.twin.idle();
      wrote[ghosts] = existsSync(file);
      await shop.close();
    } finally {
      rmSync(file, { force: true });
      delete process.env.KERN_TEST_ESCAPE_FILE;
    }
  }
  assert.deepEqual(wrote, { thread: true, process: false }, 'ghostExecArgv is trusted code: only the permission model stops it');
});
