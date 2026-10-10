import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';

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
  { key: callKey('GET', 'https://api.example/rate', null), status: 200, headers: { 'content-type': 'application/json' }, body: Buffer.from('{"rate":2}').toString('base64') },
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
