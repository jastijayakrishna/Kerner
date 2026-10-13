import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, relative as relativePath } from 'node:path';
import { pathToFileURL } from 'node:url';
import { app, production } from './helpers.js';

test('a running twin never keeps the app\'s process alive', () => {
  // An app that closes its server expects its process to end; the twin's thread and its ghosts must not hold it open.
  const file = join(tmpdir(), `kern-twin-exit-${process.pid}.mjs`);
  writeFileSync(file, `
import http from 'node:http';
import { createTwin } from ${JSON.stringify(new URL('../src/index.js', import.meta.url).href)};
import { createApp } from ${JSON.stringify(new URL('./apps/refactor/old.js', import.meta.url).href)};
const twin = await createTwin(${JSON.stringify({ old: app('refactor', 'old'), candidate: app('refactor', 'new') })});
const server = http.createServer(twin.wrap(createApp()));
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
await (await fetch('http://127.0.0.1:' + server.address().port + '/?n=2')).arrayBuffer();
await twin.idle();
server.closeAllConnections();
server.close();
console.log(JSON.stringify(twin.stats()));
`);
  try {
    const out = execFileSync(process.execPath, [file], { encoding: 'utf8', timeout: 20000 });
    assert.equal(JSON.parse(out).same, 1, out);
  } finally {
    rmSync(file, { force: true });
  }
});

test('a copy the twin cannot hand to its thread at all never ends the app: the twin switches itself off', () => {
  // Handing a copy over failed twice (the copy, then the skip that replaces it). That must never become an unhandled rejection in the
  // app's process, which Node ends by default.
  const file = join(tmpdir(), `kern-twin-handover-${process.pid}.mjs`);
  writeFileSync(file, `
import http from 'node:http';
import { Worker } from 'node:worker_threads';
import { createTwin } from ${JSON.stringify(new URL('../src/index.js', import.meta.url).href)};
import { createApp } from ${JSON.stringify(new URL('./apps/refactor/old.js', import.meta.url).href)};
const twin = await createTwin(${JSON.stringify({ old: app('refactor', 'old'), candidate: app('refactor', 'new') })});
const server = http.createServer(twin.wrap(createApp()));
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const post = Worker.prototype.postMessage;
Worker.prototype.postMessage = function () { throw new Error('the port is closed'); };
const status = (await fetch('http://127.0.0.1:' + server.address().port + '/?n=2')).status;
await new Promise((resolve) => setTimeout(resolve, 300));
Worker.prototype.postMessage = post;
const after = (await fetch('http://127.0.0.1:' + server.address().port + '/?n=3')).status;
server.closeAllConnections();
server.close();
console.log(JSON.stringify({ status, after, disabled: twin.stats().disabled }));
await twin.close();
`);
  try {
    const out = execFileSync(process.execPath, [file], { encoding: 'utf8', timeout: 20000 });
    const seen = JSON.parse(out);
    assert.deepEqual([seen.status, seen.after], [200, 200], out);
    assert.match(String(seen.disabled), /could not be handed to the twin's thread/, out);
  } finally {
    rmSync(file, { force: true });
  }
});

test('a switch the twin\'s thread cannot be told about never leaves the new version serving unwatched', () => {
  // The router has switched; telling the twin's thread fails. Serving the candidate without a thread that knows would mean nothing switches
  // it back at the next unexplained difference: the twin switches itself off instead, and the old version serves.
  const file = join(tmpdir(), `kern-twin-unwatched-${process.pid}.mjs`);
  writeFileSync(file, `
import { Worker } from 'node:worker_threads';
import { createTwin } from ${JSON.stringify(new URL('../src/index.js', import.meta.url).href)};
const twin = await createTwin(${JSON.stringify({ old: app('refactor', 'old'), candidate: app('refactor', 'new') })});
await twin.listener();
const post = Worker.prototype.postMessage;
Worker.prototype.postMessage = function () { throw new Error('the port is closed'); };
let answer;
try {
  answer = await twin.serve('candidate', 'by hand');
} catch (error) {
  answer = 'threw: ' + error.message;
}
Worker.prototype.postMessage = post;
await twin.idle();
console.log(JSON.stringify({ answer, serving: twin.stats().serving, disabled: twin.stats().disabled }));
await twin.close();
`);
  try {
    const out = execFileSync(process.execPath, [file], { encoding: 'utf8', timeout: 20000 });
    const seen = JSON.parse(out);
    assert.equal(seen.serving, 'old', out);
    assert.match(String(seen.disabled), /twin's thread/, out);
    assert.equal(typeof seen.answer, 'boolean', out);
  } finally {
    rmSync(file, { force: true });
  }
});

test('counts, differences, the decision and the evidence all come from the twin\'s own thread', async () => {
  const shop = await production('renamed-field', { epsilon: 0.5, minHours: 0 });
  await (await fetch(`${shop.url}/user`)).arrayBuffer();
  await shop.twin.idle();
  assert.equal(shop.twin.stats().different, 1);
  const [found] = shop.twin.differences();
  assert.deepEqual([found.route, found.segment, found.serving, found.outcome], ['GET /user', 'all', 'old', 'different']);
  assert.deepEqual(shop.twin.decision().reasons, ['1 unexplained difference', 'all traffic: 0 of 6 clean comparisons']);
  const evidence = shop.twin.evidence();
  assert.match(evidence.candidate, /^sha256:[0-9a-f]{64}$/);
  assert.equal(evidence.totals['*'].different, 1);
  assert.equal(evidence.unexplained[0].route, 'GET /user');
  await shop.close();
});

test('when the twin\'s thread fails, customers are served as always and the twin says why', async () => {
  process.env.KERN_TWIN_FAULT = 'thread-exit';
  try {
    const shop = await production('refactor');
    assert.equal((await fetch(`${shop.url}/?n=2`)).status, 200);
    await shop.twin.idle();
    assert.match(shop.twin.stats().disabled, /the twin's thread stopped \(exit 9\)/);
    assert.deepEqual(await (await fetch(`${shop.url}/?n=3`)).json(), { n: 3, total: 6 });
    await shop.close();
  } finally {
    delete process.env.KERN_TWIN_FAULT;
  }
});

test('waiting for the twin keeps the process alive until everything handed over is judged', () => {
  // An app that closes its server and then awaits idle() must get its answer, not a process that ends with the work half done.
  // The new version answers 300 ms late, so the twin is still judging after the server has closed.
  const file = join(tmpdir(), `kern-twin-idle-${process.pid}.mjs`);
  writeFileSync(file, `
import http from 'node:http';
import { createTwin } from ${JSON.stringify(new URL('../src/index.js', import.meta.url).href)};
import { createApp } from ${JSON.stringify(new URL('./apps/slow/old.js', import.meta.url).href)};
const twin = await createTwin(${JSON.stringify({ old: app('slow', 'old'), candidate: app('slow', 'new') })});
const server = http.createServer(twin.wrap(createApp()));
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
for (let n = 1; n <= 3; n++) await (await fetch('http://127.0.0.1:' + server.address().port + '/?n=' + n)).arrayBuffer();
server.closeAllConnections();
server.close();
await twin.idle();
console.log(JSON.stringify(twin.stats()));
await twin.close();
`);
  try {
    const out = execFileSync(process.execPath, [file], { encoding: 'utf8', timeout: 20000 });
    assert.equal(JSON.parse(out).same, 3, out);
  } finally {
    rmSync(file, { force: true });
  }
});

test('a version named by a path relative to the working folder, or by a file URL, loads in the ghosts as it does in the app', async () => {
  const relative = (version) => ({ ...version, module: relativePath(process.cwd(), version.module) });
  const url = (version) => ({ ...version, module: pathToFileURL(version.module).href });
  for (const name of [relative, url]) {
    const shop = await production('refactor', { old: name(app('refactor', 'old')), candidate: name(app('refactor', 'new')) });
    assert.equal(shop.twin.stats().disabled, null, shop.twin.stats().disabled);
    assert.deepEqual(await (await fetch(`${shop.url}/?n=5`)).json(), { n: 5, total: 15 });
    await shop.twin.idle();
    assert.equal(shop.twin.stats().same, 1, JSON.stringify(shop.twin.stats()));
    await shop.close();
  }
});
