import net from 'node:net';
import dgram from 'node:dgram';
import fs from 'node:fs';
import childProcess from 'node:child_process';
import workerThreads from 'node:worker_threads';
import { syncBuiltinESMExports } from 'node:module';
import { callKey, isWrite } from './keys.js';

// What a ghost may do: have its fetch calls answered from the real run's recording, and nothing else. Writes are written down and
// never done; any other way out (sockets, UDP, programs, threads, file writes, opening files to write) is refused, so a ghost
// cannot touch anything real through Node's own APIs.
let job = null;

/** Starts a request: the real run's calls, queued by name, so each recorded answer is used once. */
export function beginJob(io) {
  const queues = new Map();
  for (const call of io ?? []) {
    if (!queues.has(call.key)) queues.set(call.key, []);
    queues.get(call.key).push(call);
  }
  job = { queues, effects: [], events: [] };
}

/** Ends a request: what the ghost tried to write, and what it was refused or could not find. */
export function endJob() {
  const out = job ? { effects: job.effects, events: job.events } : { effects: [], events: [] };
  job = null;
  return out;
}

function note(type, detail) {
  if (job && job.events.length < 50) job.events.push({ type, detail: String(detail).slice(0, 300) });
}

function wrote(effect) {
  if (job && job.effects.length < 200) job.effects.push(effect);
}

const NULL_BODY = new Set([101, 204, 205, 304]);
const loopback = (host) => ['127.0.0.1', 'localhost', '::1', '[::1]'].includes(String(host));
const refusal = (what) => Object.assign(new Error(`${what} is refused in a Kern ghost run`), { code: 'EACCES' });
const { O_WRONLY, O_RDWR, O_CREAT, O_TRUNC, O_APPEND } = fs.constants;
// Only 'r', 'rs' and 'sr' open a file just to read it; every other flag can change it ('w' already empties it).
const opensToWrite = (flags) => (typeof flags === 'number' ? (flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC | O_APPEND)) !== 0 : !/^(r|rs|sr)$/.test(String(flags ?? 'r')));

/** Seals this thread. `harnessPort` is the loopback port of the ghost's own server: the one connection it may make. */
export function installGhostIO({ harnessPort }) {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async function ghostFetch(input, init) {
    const request = new Request(input, init);
    const url = new URL(request.url);
    if (loopback(url.hostname) && Number(url.port) === harnessPort) return realFetch(request);
    const body = request.body ? Buffer.from(await request.arrayBuffer()) : Buffer.alloc(0);
    const key = callKey(request.method, request.url, body);
    const write = isWrite(request.method);
    if (write) wrote(key);
    const recorded = job?.queues.get(key)?.shift();
    if (!recorded) {
      note(write ? 'unmatched-write' : 'unmatched-read', `${request.method.toUpperCase()} ${request.url}`);
      throw new TypeError('fetch failed');
    }
    if (recorded.error) throw new TypeError('fetch failed');
    const headers = Object.entries(recorded.headers ?? {}).filter(([name]) => !['content-encoding', 'content-length', 'transfer-encoding'].includes(name));
    const answer = NULL_BODY.has(recorded.status) || recorded.body == null ? null : Buffer.from(recorded.body, 'base64');
    return new Response(answer, { status: recorded.status, headers });
  };

  const realConnect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function ghostConnect(...args) {
    const first = Array.isArray(args[0]) ? args[0][0] : args[0];
    const options = first !== null && typeof first === 'object' ? first : { port: first, host: typeof args[1] === 'string' ? args[1] : 'localhost' };
    // Node's HTTP client always passes `path: null` (its agent's way of saying "not a pipe"), so null counts as no path too.
    if (options.path == null && loopback(options.host ?? 'localhost') && Number(options.port) === harnessPort) return realConnect.apply(this, args);
    note('denied', `a network connection to ${options.path ?? `${options.host ?? 'localhost'}:${options.port}`}`);
    process.nextTick(() => this.destroy(refusal('A network connection')));
    return this;
  };

  // The app may start its own server when it loads; in a ghost that server never opens. The harness's server is already listening.
  net.Server.prototype.listen = function ghostListen(...args) {
    const done = args.find((a) => typeof a === 'function');
    if (done) process.nextTick(done);
    return this;
  };

  dgram.Socket.prototype.send = function ghostSend(...args) {
    note('denied', 'a UDP message');
    const done = args.find((a) => typeof a === 'function');
    if (done) process.nextTick(done, refusal('A UDP message'));
  };

  for (const name of ['spawn', 'spawnSync', 'exec', 'execSync', 'execFile', 'execFileSync', 'fork']) {
    childProcess[name] = function ghostProgram(file) {
      note('denied', `starting the program ${String(file).trim().split(/\s+/)[0]}`);
      throw refusal('Starting a program');
    };
  }

  // A thread the app starts would begin outside this seal, free to reach anything real.
  workerThreads.Worker = function ghostWorker() {
    note('denied', 'starting a worker thread');
    throw refusal('Starting a worker thread');
  };

  // Opening a file to write can already destroy it, so a ghost may open files only to read them.
  const { open, openSync } = fs;
  const promisesOpen = fs.promises.open;
  const opened = (target) => wrote(`FS open ${String(target)}`);
  fs.openSync = function ghostOpenSync(target, flags, ...rest) {
    if (!opensToWrite(flags)) return openSync.call(this, target, flags, ...rest);
    opened(target);
    throw refusal('Opening a file to write');
  };
  fs.open = function ghostOpen(target, flags, ...rest) {
    if (typeof flags === 'function' || !opensToWrite(flags)) return open.call(this, target, flags, ...rest);
    opened(target);
    const done = rest.findLast((a) => typeof a === 'function');
    if (done) process.nextTick(done, refusal('Opening a file to write'));
  };
  fs.promises.open = async function ghostPromisesOpen(target, flags, ...rest) {
    if (!opensToWrite(flags)) return promisesOpen.call(this, target, flags, ...rest);
    opened(target);
    throw refusal('Opening a file to write');
  };

  for (const name of ['writeFile', 'appendFile', 'unlink', 'rm', 'rmdir', 'rename', 'mkdir', 'mkdtemp', 'copyFile', 'cp', 'truncate', 'symlink', 'link', 'chmod', 'chown', 'lchown', 'utimes', 'lutimes']) {
    const effect = (target) => wrote(`FS ${name} ${String(target)}`);
    fs[`${name}Sync`] = (target) => {
      effect(target);
      throw refusal(`fs.${name}`);
    };
    fs[name] = (target, ...rest) => {
      effect(target);
      const done = rest.findLast((a) => typeof a === 'function');
      if (done) process.nextTick(done, refusal(`fs.${name}`));
    };
    fs.promises[name] = async (target) => {
      effect(target);
      throw refusal(`fs.${name}`);
    };
  }
  fs.createWriteStream = (target) => {
    wrote(`FS createWriteStream ${String(target)}`);
    throw refusal('fs.createWriteStream');
  };
  syncBuiltinESMExports();
}
