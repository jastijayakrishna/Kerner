import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const source = (file) => JSON.stringify(new URL(`../src/${file}`, import.meta.url).href);
const script = `
import { installGhostIO, beginJob, endJob } from ${source('ghost-io.js')};
import { callKey } from ${source('keys.js')};
import net from 'node:net';
import fs from 'node:fs';
import { execSync } from 'node:child_process';
installGhostIO({ harnessPort: 1 });
const body = Buffer.from(JSON.stringify({ amount: 5 }));
const file = '/tmp/kern-ghost-' + process.pid;
beginJob([
  { key: callKey('GET', 'https://api.example/rate', null), status: 200, headers: { 'content-type': 'application/json' }, body: new Uint8Array(Buffer.from('{"rate":2}')) },
  { key: callKey('POST', 'https://pay.example/charge', body), status: 200, headers: {}, body: null },
]);
const out = {};
out.rate = await (await fetch('https://api.example/rate')).json();
out.charge = (await fetch('https://pay.example/charge', { method: 'POST', body })).status;
out.second = await fetch('https://pay.example/charge', { method: 'POST', body }).then(() => 'answered', (e) => e.message);
out.unknown = await fetch('https://elsewhere.example/').then(() => 'answered', (e) => e.message);
out.socket = await new Promise((resolve) => net.connect(5432, 'db.example').on('error', (e) => resolve(e.code)));
try { execSync('echo hi'); out.exec = 'ran'; } catch (e) { out.exec = e.code; }
try { fs.writeFileSync(file, 'x'); out.write = 'wrote'; } catch (e) { out.write = e.code; }
out.writeAsync = await fs.promises.writeFile(file, 'x').then(() => 'wrote', (e) => e.code);
out.exists = fs.existsSync(file);
Object.assign(out, endJob());
console.log(JSON.stringify(out));
`;

test('a ghost gets its calls answered from the recording and does nothing real', () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8' }));
  assert.deepEqual(out.rate, { rate: 2 });
  assert.equal(out.charge, 200);
  assert.equal(out.second, 'fetch failed');
  assert.equal(out.unknown, 'fetch failed');
  assert.equal(out.socket, 'EACCES');
  assert.equal(out.exec, 'EACCES');
  assert.equal(out.write, 'EACCES');
  assert.equal(out.writeAsync, 'EACCES');
  assert.equal(out.exists, false);
  assert.equal(out.effects.filter((e) => e.startsWith('POST https://pay.example/charge')).length, 2);
  assert.equal(out.effects.filter((e) => e.startsWith('FS writeFile')).length, 2);
  assert.deepEqual(out.events.map((e) => e.type), ['unmatched-write', 'unmatched-read', 'denied', 'denied']);
});

test('notes about loose matches never crowd out the calls a verdict depends on', () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob, endJob } from ${source('ghost-io.js')};
import { callKey } from ${source('keys.js')};
installGhostIO({ harnessPort: 1 });
const recorded = Buffer.from('recorded');
beginJob(Array.from({ length: 60 }, () => ({ key: callKey('POST', 'https://queue.example/poll', recorded), status: 200, headers: {}, body: null })));
for (let i = 0; i < 60; i++) await fetch('https://queue.example/poll', { method: 'POST', body: 'live ' + i });
const missed = await fetch('https://elsewhere.example/').then(() => 'answered', (e) => e.message);
const events = endJob().events;
console.log(JSON.stringify({ missed, loose: events.filter((e) => e.type === 'loose').length, unmatched: events.filter((e) => e.type === 'unmatched-read').map((e) => e.detail) }));
`], { encoding: 'utf8' }));
  assert.equal(out.missed, 'fetch failed');
  assert.deepEqual(out.unmatched, ['GET https://elsewhere.example/'], 'the call the recording could not answer is always written down');
  assert.ok(out.loose > 0 && out.loose <= 50, `${out.loose} loose notes, at most 50 kept`);
});

test("a call about the connection or the server is answered as often as it is asked, even from an earlier job's recording; a data read only once", () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { beginJob, endJob, take } from ${source('ghost-io.js')};
import { pgKey, redisKey } from ${source('keys.js')};
const version = { key: pgKey('select version();', []), result: { rows: [{ version: 'PostgreSQL 17.6' }] } };
const named = { key: redisKey('CLIENT', ['SETNAME', 'bull:1']), reply: 'OK' };
const row = { key: pgKey('select * from users where id = $1', [1]), result: { rows: [{ id: 1 }] } };
const answered = (call) => (call ? call.key.split(' ')[0] : null);
beginJob([version, named, row]);
const first = [take(version.key), take(version.key), take(redisKey('CLIENT', ['SETNAME', 'bull:2'])), take(redisKey('CLIENT', ['SETNAME', 'bull:3'])), take(row.key), take(row.key)].map(answered);
endJob();
beginJob([]);
const later = [take(pgKey('select version();', [])), take(redisKey('CLIENT', ['SETNAME', 'bull:4'])), take(row.key)].map(answered);
endJob();
console.log(JSON.stringify({ first, later }));
`], { encoding: 'utf8' }));
  assert.deepEqual(out.first, ['PG', 'PG', 'REDIS', 'REDIS', 'PG', null], 'the version check and the connection name repeat; the data read is used once');
  assert.deepEqual(out.later, ['PG', 'REDIS', null], 'session answers seen before serve a later job; data never carries over');
});

test('a ghost may not start a thread of its own, since that thread would run outside the seal', () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob, endJob } from ${source('ghost-io.js')};
import { Worker } from 'node:worker_threads';
installGhostIO({ harnessPort: 1 });
beginJob([]);
let thread;
try { new Worker('process.exit(0)', { eval: true }); thread = 'started'; } catch (e) { thread = e.code; }
console.log(JSON.stringify({ thread, events: endJob().events.map((e) => e.type) }));
`], { encoding: 'utf8' }));
  assert.deepEqual(out, { thread: 'EACCES', events: ['denied'] });
});

test('a ghost may open a file to read it, never to write it, since opening to write can already empty it', () => {
  const file = join(tmpdir(), `kern-ghost-open-${process.pid}.txt`);
  writeFileSync(file, 'real data');
  try {
    const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob, endJob } from ${source('ghost-io.js')};
import fs from 'node:fs';
installGhostIO({ harnessPort: 1 });
beginJob([]);
const file = ${JSON.stringify(file)};
const out = {};
const fd = fs.openSync(file, 'r');
out.read = fs.readFileSync(fd, 'utf8');
fs.closeSync(fd);
try { fs.openSync(file, 'w'); out.openSync = 'opened'; } catch (e) { out.openSync = e.code; }
out.open = await new Promise((resolve) => fs.open(file, 'r+', (e) => resolve(e ? e.code : 'opened')));
out.promises = await fs.promises.open(file, 'a').then(() => 'opened', (e) => e.code);
out.effects = endJob().effects;
console.log(JSON.stringify(out));
`], { encoding: 'utf8' }));
    assert.equal(out.read, 'real data');
    assert.deepEqual([out.openSync, out.open, out.promises], ['EACCES', 'EACCES', 'EACCES']);
    assert.equal(out.effects.filter((e) => e.startsWith('FS open')).length, 3);
    assert.equal(readFileSync(file, 'utf8'), 'real data');
  } finally {
    rmSync(file, { force: true });
  }
});

test('a ghost may not signal a process, load a native add-on, reach Node\'s internals, change a priority, look up a name or start WASI', () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob, endJob } from ${source('ghost-io.js')};
import dns from 'node:dns';
import os from 'node:os';
installGhostIO({ harnessPort: 1 });
beginJob([]);
const code = (f) => { try { f(); return 'allowed'; } catch (e) { return e.code; } };
const out = {};
out.kill = code(() => process.kill(process.pid, 0));
out.addon = code(() => process.dlopen({ exports: {} }, '/app/node_modules/sharp/build/Release/sharp.node'));
out.binding = code(() => process.binding('tcp_wrap'));
out.priority = code(() => os.setPriority(0, 0));
out.lookup = await new Promise((resolve) => dns.lookup('example.com', (e) => resolve(e ? e.code : 'allowed')));
out.resolve = await dns.promises.resolve4('example.com').then(() => 'allowed', (e) => e.code);
const { WASI } = await import('node:wasi');
out.wasi = code(() => new WASI({ version: 'preview1' }));
out.events = endJob().events.map((e) => e.type);
console.log(JSON.stringify(out));
`], { encoding: 'utf8' }));
  assert.deepEqual([out.kill, out.addon, out.binding, out.priority, out.lookup, out.resolve, out.wasi], Array(7).fill('EACCES'));
  assert.deepEqual(out.events, Array(7).fill('denied'));
});

test('a ghost may load the native add-ons of the packages it was allowed, and no others', () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob } from ${source('ghost-io.js')};
installGhostIO({ harnessPort: 1, addons: ['bcrypt', '@node-rs/argon2'] });
beginJob([]);
const code = (f) => { try { f(); return 'allowed'; } catch (e) { return e.code; } };
console.log(JSON.stringify({
  bcrypt: code(() => process.dlopen({ exports: {} }, '/app/node_modules/bcrypt/lib/binding/napi-v3/bcrypt_lib.node')),
  scoped: code(() => process.dlopen({ exports: {} }, 'C:\\\\app\\\\node_modules\\\\@node-rs\\\\argon2\\\\argon2.node')),
  sharp: code(() => process.dlopen({ exports: {} }, '/app/node_modules/sharp/build/Release/sharp.node')),
}));
`], { encoding: 'utf8' }));
  assert.deepEqual(out, { bcrypt: 'ERR_DLOPEN_FAILED', scoped: 'ERR_DLOPEN_FAILED', sharp: 'EACCES' });
});

test('a ghost may not write to a file descriptor it did not open', () => {
  const file = join(tmpdir(), `kern-ghost-fd-${process.pid}.txt`);
  writeFileSync(file, 'real data');
  try {
    const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob } from ${source('ghost-io.js')};
import fs from 'node:fs';
const fd = fs.openSync(${JSON.stringify(file)}, 'r+');
installGhostIO({ harnessPort: 1 });
beginJob([]);
const code = (f) => { try { f(); return 'allowed'; } catch (e) { return e.code; } };
const out = {};
out.writeSync = code(() => fs.writeSync(fd, 'ghost'));
out.write = await new Promise((resolve) => fs.write(fd, 'ghost', (e) => resolve(e ? e.code : 'allowed')));
out.truncate = code(() => fs.ftruncateSync(fd, 0));
out.stdout = code(() => fs.writeSync(1, ''));
console.log(JSON.stringify(out));
`], { encoding: 'utf8' }));
    assert.deepEqual(out, { writeSync: 'EACCES', write: 'EACCES', truncate: 'EACCES', stdout: 'allowed' });
    assert.equal(readFileSync(file, 'utf8'), 'real data');
  } finally {
    rmSync(file, { force: true });
  }
});

test('a ghost counts every write it makes, by key, and says when it made more different ones than it counts', () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob, endJob, wrote } from ${source('ghost-io.js')};
installGhostIO({ ports: [1] });
beginJob([]);
for (let i = 0; i < 300; i++) wrote('POST https://rows.example/ a');
wrote('POST https://rows.example/ b');
const some = endJob();
beginJob([]);
for (let i = 0; i <= 10000; i++) wrote('POST https://rows.example/ ' + i);
const many = endJob();
console.log(JSON.stringify({ listed: some.effects.length, writes: some.writes, tooMany: some.tooMany, counted: many.writes.length, overflow: many.tooMany }));
`], { encoding: 'utf8' }));
  assert.deepEqual(out, {
    listed: 200,
    writes: [['POST https://rows.example/ a', 300], ['POST https://rows.example/ b', 1]],
    tooMany: false,
    counted: 10000,
    overflow: true,
  });
});

test('a call whose payload carried the clock gets the recorded call of the same shape, once', () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', `
import { installGhostIO, beginJob, endJob } from ${source('ghost-io.js')};
import { callKey } from ${source('keys.js')};
installGhostIO({ ports: [1] });
beginJob([{ key: callKey('POST', 'https://api.example/events', Buffer.from('{"at":1}')), status: 201, headers: {}, body: null }]);
const out = {};
out.first = (await fetch('https://api.example/events', { method: 'POST', body: '{"at":2}' })).status;
out.second = await fetch('https://api.example/events', { method: 'POST', body: '{"at":3}' }).then(() => 'answered', (e) => e.message);
const io = endJob();
out.events = io.events.map((e) => e.type);
out.effects = io.effects.length;
console.log(JSON.stringify(out));
`], { encoding: 'utf8' }));
  assert.deepEqual(out, { first: 201, second: 'fetch failed', events: ['loose', 'unmatched-write'], effects: 2 });
});
