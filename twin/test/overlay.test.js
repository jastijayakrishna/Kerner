import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const source = (file) => JSON.stringify(new URL(`../src/${file}`, import.meta.url).href);

test('with the overlay, a ghost\'s file writes land in memory: written down, read back, never on disk, and forgotten at the next request', () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-overlay-'));
  writeFileSync(join(dir, 'real.txt'), 'real');
  try {
    const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob, endJob } from ${source('ghost-io.js')};
import fs from 'node:fs';
import { join } from 'node:path';
const dir = ${JSON.stringify(dir)};
installGhostIO({ ports: [1], overlay: true });
beginJob([]);
const out = {};
fs.writeFileSync(join(dir, 'a.txt'), 'one');
fs.appendFileSync(join(dir, 'a.txt'), ' two');
out.read = fs.readFileSync(join(dir, 'a.txt'), 'utf8');
await fs.promises.mkdir(join(dir, 'sub', 'deep'), { recursive: true });
await new Promise((resolve) => fs.writeFile(join(dir, 'sub', 'b.txt'), 'b', resolve));
fs.renameSync(join(dir, 'real.txt'), join(dir, 'moved.txt'));
out.list = fs.readdirSync(dir);
out.exists = [fs.existsSync(join(dir, 'real.txt')), fs.existsSync(join(dir, 'moved.txt')), fs.existsSync(join(dir, 'sub', 'deep'))];
out.moved = fs.readFileSync(join(dir, 'moved.txt'), 'utf8');
out.size = fs.statSync(join(dir, 'a.txt')).size;
out.stream = await new Promise((resolve) => { const s = fs.createWriteStream(join(dir, 'log.txt')); s.end('logged', () => resolve(fs.readFileSync(join(dir, 'log.txt'), 'utf8'))); });
try { fs.symlinkSync(join(dir, 'a.txt'), join(dir, 'link')); out.symlink = 'made'; } catch (e) { out.symlink = e.code; }
out.effects = endJob().effects.map((e) => e.split(' ')[1]);
beginJob([]);
out.after = [fs.existsSync(join(dir, 'a.txt')), fs.existsSync(join(dir, 'real.txt'))];
console.log(JSON.stringify(out));
`], { encoding: 'utf8' }));
    assert.equal(out.read, 'one two');
    assert.deepEqual(out.list, ['a.txt', 'moved.txt', 'sub']);
    assert.deepEqual(out.exists, [false, true, true]);
    assert.equal(out.moved, 'real');
    assert.equal(out.size, 7);
    assert.equal(out.stream, 'logged');
    assert.equal(out.symlink, 'EACCES', 'what the overlay does not cover is still refused');
    assert.deepEqual(out.effects, ['writeFile', 'appendFile', 'mkdir', 'writeFile', 'rename', 'createWriteStream', 'symlink']);
    assert.deepEqual(out.after, [false, true], 'the next request starts from the real disk');
    assert.deepEqual(readdirSync(dir), ['real.txt'], 'nothing was written');
    assert.equal(readFileSync(join(dir, 'real.txt'), 'utf8'), 'real');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
