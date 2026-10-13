import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { current, loadRecord } from '../src/capture.js';
import pgAdapter from '../src/adapters/pg.js';
import ioredisAdapter from '../src/adapters/ioredis.js';

// The recorders run in the app's own thread: whatever they cannot copy, the app must see exactly what it would see without the twin.

/** A folder with an app file and stand-ins for client libraries in its node_modules; answers a require from that app. */
function appWith(packages) {
  const dir = mkdtempSync(join(tmpdir(), 'kern-twin-recording-'));
  for (const [name, source] of Object.entries(packages)) {
    mkdirSync(join(dir, 'node_modules', name), { recursive: true });
    writeFileSync(join(dir, 'node_modules', name, 'index.js'), source);
  }
  writeFileSync(join(dir, 'app.js'), '');
  return { appRequire: createRequire(join(dir, 'app.js')), remove: () => rmSync(dir, { recursive: true, force: true }) };
}

// Like pg: a value it cannot send comes back as a rejected promise (or through the callback), never as a throw from query().
const PG = `
class Client {
  constructor() { this._Promise = Promise; this.calls = 0; }
  query(config, values, callback) {
    this.calls++;
    let error = null;
    try { JSON.stringify(values); } catch (e) { error = e; }
    if (typeof callback === 'function') { process.nextTick(callback, error, error ? undefined : { rows: [] }); return undefined; }
    return error ? Promise.reject(error) : Promise.resolve({ command: 'SELECT', rowCount: 0, rows: [], fields: [] });
  }
}
module.exports = { Client };
`;

// Like ioredis: every command goes through sendCommand, which answers the command's promise.
const IOREDIS = `
class Redis {
  constructor() { this.sent = 0; }
  sendCommand(command) { this.sent++; command.reject(new TypeError('Cannot convert object to primitive value')); return command.promise; }
}
module.exports = Redis;
module.exports.default = Redis;
`;

test('a Postgres call the twin cannot name goes to pg untouched, and its request is skipped instead of broken', async () => {
  const app = appWith({ pg: PG });
  try {
    pgAdapter.record({ appRequire: app.appRequire, current, maxBodyBytes: 1 << 20 });
    const { Client } = app.appRequire('pg');
    const client = new Client();
    const record = loadRecord();
    const circular = {};
    circular.self = circular;
    const answer = current.run(record, () => client.query('SELECT $1::jsonb', [circular]));
    await assert.rejects(answer, /circular/i, 'the app gets pg\'s own answer: a rejected promise, not a throw from the twin');
    assert.equal(client.calls, 1);
    assert.match(String(record.problem), /Postgres call Kern could not name/);
  } finally {
    app.remove();
  }
});

test('a Redis command the twin cannot name goes to ioredis untouched, and its request is skipped instead of broken', async () => {
  const app = appWith({ ioredis: IOREDIS });
  try {
    ioredisAdapter.record({ appRequire: app.appRequire, current, maxBodyBytes: 1 << 20 });
    const Redis = app.appRequire('ioredis');
    const redis = new Redis();
    const record = loadRecord();
    let resolve;
    let reject;
    const promise = new Promise((yes, no) => {
      resolve = yes;
      reject = no;
    });
    const command = { name: 'set', args: ['key', Object.create(null)], promise, resolve, reject };
    const answer = current.run(record, () => redis.sendCommand(command));
    await assert.rejects(answer, /primitive/, 'the app gets ioredis\'s own answer');
    assert.equal(redis.sent, 1);
    assert.match(String(record.problem), /Redis command Kern could not name/);
  } finally {
    app.remove();
  }
});
