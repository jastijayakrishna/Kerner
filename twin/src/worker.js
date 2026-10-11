import { parentPort, workerData } from 'node:worker_threads';
import http from 'node:http';
import fs from 'node:fs';
import { devNull } from 'node:os';
import { syncBuiltinESMExports } from 'node:module';
import { pathToFileURL } from 'node:url';
import { installWorld, resetWorld } from './world.js';
import { installGhostIO, beginJob, endJob } from './ghost-io.js';
import { exactBytes } from './serialize.js';

// A ghost: one version of the app in its own thread, answering copies of real requests in a sealed world. Its own server listens on
// loopback before the seal goes on; after that, the app can reach nothing but that server.
const realRequest = http.request;
let handler = null;
const server = http.createServer((req, res) => handler(req, res));
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const port = server.address().port;

installWorld();
// Before the seal, which would refuse the null device muteOutput() opens to write to.
muteOutput();
installGhostIO({ harnessPort: port });
const app = await import(pathToFileURL(workerData.module).href);
const factory = app[workerData.export];
if (typeof factory !== 'function') throw new Error(`has no function named ${workerData.export}`);
handler = await factory();
if (typeof handler !== 'function') throw new Error(`${workerData.export}() must return a request listener: (req, res) => { ... }`);

/** A ghost's output is not the app's: it is dropped here, never mixed into production logs. */
function muteOutput() {
  // The console and process.stdout: callers waiting for a write to finish are still told it did.
  for (const stream of [process.stdout, process.stderr]) {
    stream.write = (chunk, encoding, callback) => {
      const done = typeof encoding === 'function' ? encoding : callback;
      if (typeof done === 'function') process.nextTick(done);
      return true;
    };
  }
  // Loggers such as pino write straight to file descriptors 1 and 2, past the console; a worker shares them with the app. In a
  // ghost those writes go to the null device, so each call still gives its usual result.
  const sink = fs.openSync(devNull, 'w');
  for (const name of ['write', 'writeSync', 'writev', 'writevSync']) {
    const real = fs[name];
    fs[name] = function ghostWrite(fd, ...rest) {
      return real.call(this, fd === 1 || fd === 2 ? sink : fd, ...rest);
    };
  }
  syncBuiltinESMExports();
}

let finish = null;
function crashed(error) {
  // The app's state may now be broken: the coordinator restarts both ghosts after this request.
  parentPort.postMessage({ type: 'dirty' });
  const message = `uncaught ${error?.name ?? 'error'}: ${String(error?.message ?? error).slice(0, 300)}`;
  if (finish) {
    const done = finish;
    finish = null;
    done({ error: message });
  }
}
process.on('uncaughtException', crashed);
process.on('unhandledRejection', crashed);

parentPort.on('message', (message) => {
  if (message?.type === 'job') run(message.id, message.job);
});
parentPort.postMessage({ type: 'ready' });

async function run(id, job) {
  resetWorld(job.start, job.seed);
  beginJob(job.io);
  const result = await new Promise((resolve) => {
    finish = resolve;
    const sent = job.request.body ?? new Uint8Array();
    const body = Buffer.from(sent.buffer, sent.byteOffset, sent.byteLength);
    const headers = { ...job.request.headers, 'content-length': String(body.length) };
    const req = realRequest({ host: '127.0.0.1', port, method: job.request.method, path: job.request.url, headers, agent: false }, (res) => {
      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: exactBytes(chunks) }));
      res.on('error', (error) => resolve({ error: `the answer broke off: ${error.message}` }));
    });
    req.on('error', (error) => resolve({ error: `no answer: ${error.message}` }));
    req.end(body);
  });
  finish = null;
  const io = endJob();
  parentPort.postMessage({
    type: 'result', id,
    result: { status: result.status, headers: result.headers, error: result.error, body: result.body ?? null, effects: io.effects, events: io.events },
  });
}
