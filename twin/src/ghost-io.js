import net from 'node:net';
import dgram from 'node:dgram';
import dns from 'node:dns';
import fs from 'node:fs';
import os from 'node:os';
import childProcess from 'node:child_process';
import workerThreads from 'node:worker_threads';
import { syncBuiltinESMExports } from 'node:module';
import { callKey, isWrite, readAllowed, shapeOf } from './keys.js';
import { installOverlay, resetOverlay } from './overlay.js';
import { mine, never } from './turn.js';

// What a ghost may do: have its calls answered from the real run's recording, and nothing else. Writes are written down and never
// done. Every other way out is refused, so a ghost cannot touch anything real through Node's own APIs: network connections, UDP,
// DNS, programs, threads, signals, native add-ons (but those allowed by name), Node's internal bindings, WASI, process priorities,
// file writes, opening files to write, and writing to file descriptors it did not open.
let job = null;

/** Starts a request: the real run's calls, in the order they were made; each recorded answer is used once. */
export function beginJob(io) {
  resetOverlay();
  job = { calls: (io ?? []).map((call) => ({ call, shape: shapeOf(call.key), used: false })), effects: [], events: [] };
}

/** Ends a request: what the ghost tried to write, and what it was refused or could not find. */
export function endJob() {
  const out = job ? { effects: job.effects, events: job.events } : { effects: [], events: [] };
  job = null;
  return out;
}

/**
 * The recorded answer to a call: the unused one with exactly this key, else the next unused one with the same shape (its payload carried
 * the clock or a random id, which a ghost cannot reproduce exactly; such a match is noted as 'loose'). Undefined when there is none.
 */
export function take(key) {
  if (!job || !mine()) return undefined;
  const shape = shapeOf(key);
  const found = job.calls.find((entry) => !entry.used && entry.call.key === key) ?? job.calls.find((entry) => !entry.used && entry.shape === shape);
  if (!found) return undefined;
  found.used = true;
  if (found.call.key !== key) note('loose', found.call.key);
  return found.call;
}

/** Something the ghost was refused ('denied'), or a call the recording could not answer ('unmatched-read', 'unmatched-write'). */
export function note(type, detail) {
  if (!mine()) return;
  if (job && job.events.length < 50) job.events.push({ type, detail: String(detail).slice(0, 300) });
}

/** A write the ghost made: written down, never done. */
export function wrote(effect) {
  if (!mine()) return;
  if (job && job.effects.length < 200) job.effects.push(effect);
}

const NULL_BODY = new Set([101, 204, 205, 304]);
const loopback = (host) => ['127.0.0.1', 'localhost', '::1', '[::1]'].includes(String(host));
const refusal = (what) => Object.assign(new Error(`${what} is refused in a Kern ghost run`), { code: 'EACCES' });
const { O_WRONLY, O_RDWR, O_CREAT, O_TRUNC, O_APPEND } = fs.constants;
// Only 'r', 'rs' and 'sr' open a file just to read it; every other flag can change it ('w' already empties it).
const opensToWrite = (flags) => (typeof flags === 'number' ? (flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC | O_APPEND)) !== 0 : !/^(r|rs|sr)$/.test(String(flags ?? 'r')));
/** Refuses a call made in callback style: its callback gets the refusal, as it would a real failure. */
function refuseLater(args, what) {
  const done = args.findLast((a) => typeof a === 'function');
  if (done) process.nextTick(done, refusal(what));
}
/** The package a native add-on file belongs to: the last node_modules/<name> (or @scope/name) on its path. */
export const packageOf = (file) => String(file).replaceAll('\\', '/').match(/.*node_modules\/((?:@[^/]+\/)?[^/]+)\//)?.[1] ?? null;
const LOOKUPS = ['lookup', 'lookupService', 'resolve', 'resolve4', 'resolve6', 'resolveAny', 'resolveCaa', 'resolveCname', 'resolveMx', 'resolveNaptr', 'resolveNs', 'resolvePtr', 'resolveSoa', 'resolveSrv', 'resolveTxt', 'reverse'];

/**
 * Seals this thread. `ports` are the loopback ports of the ghost's own servers (the harness, and the http replay of Phase 4): the only
 * connections it may make. `harnessPort` alone still works. `addons` names the packages whose native add-ons may load: pure computation
 * only (bcrypt, say), since an add-on acts below every control here.
 */
export function installGhostIO({ harnessPort, ports = [harnessPort], addons = [], realReads = [], ask = null, overlay = false }) {
  const allowed = new Set(ports.filter((port) => Number.isInteger(port)));
  const realFetch = globalThis.fetch;
  globalThis.fetch = async function ghostFetch(input, init) {
    const request = new Request(input, init);
    const url = new URL(request.url);
    if (loopback(url.hostname) && allowed.has(Number(url.port))) return realFetch(request);
    // A call the current turn did not make waits forever (turn.js): no answer, nothing written down.
    if (!mine()) return never();
    const body = request.body ? Buffer.from(await request.arrayBuffer()) : Buffer.alloc(0);
    const key = callKey(request.method, request.url, body);
    const write = isWrite(request.method);
    if (write) wrote(key);
    let recorded = take(key);
    if (!recorded && !write && ask && readAllowed(realReads, request.method, request.url)) {
      // An allow-listed read the recording cannot answer: the twin's thread makes it for real, once for both ghosts of this request.
      const answer = await ask({ type: 'real-read', method: request.method.toUpperCase(), url: request.url, headers: Object.fromEntries(request.headers) });
      if (!answer.error) {
        note('real-read', `${request.method.toUpperCase()} ${request.url}`);
        recorded = answer;
      }
    }
    if (!recorded) {
      note(write ? 'unmatched-write' : 'unmatched-read', `${request.method.toUpperCase()} ${request.url}`);
      throw new TypeError('fetch failed');
    }
    if (recorded.error) throw new TypeError('fetch failed');
    const headers = Object.entries(recorded.headers ?? {}).filter(([name]) => !['content-encoding', 'content-length', 'transfer-encoding'].includes(name));
    const answer = NULL_BODY.has(recorded.status) || recorded.body == null ? null : recorded.body;
    return new Response(answer, { status: recorded.status, headers });
  };

  const realConnect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function ghostConnect(...args) {
    const first = Array.isArray(args[0]) ? args[0][0] : args[0];
    const options = first !== null && typeof first === 'object' ? first : { port: first, host: typeof args[1] === 'string' ? args[1] : 'localhost' };
    // Node's HTTP client always passes `path: null` (its agent's way of saying "not a pipe"), so null counts as no path too.
    if (options.path == null && loopback(options.host ?? 'localhost') && allowed.has(Number(options.port))) return realConnect.apply(this, args);
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

  // A name lookup is itself a message to the outside world (a name can carry data), and its answer is not in the recording.
  const looked = (target) => note('denied', `a DNS lookup of ${String(target)}`);
  for (const owner of [dns, dns.Resolver.prototype]) {
    for (const name of LOOKUPS.filter((n) => typeof owner[n] === 'function')) {
      owner[name] = function ghostLookup(target, ...rest) {
        looked(target);
        refuseLater(rest, 'A DNS lookup');
      };
    }
  }
  for (const owner of [dns.promises, dns.promises.Resolver.prototype]) {
    for (const name of LOOKUPS.filter((n) => typeof owner[n] === 'function')) {
      owner[name] = async function ghostLookup(target) {
        looked(target);
        throw refusal('A DNS lookup');
      };
    }
  }

  // Inside a thread, process.kill reaches the whole process: an app's shutdown path that signals itself would end production.
  process.kill = function ghostKill(pid, signal) {
    note('denied', `sending ${signal ?? 'SIGTERM'} to process ${pid}`);
    throw refusal('Sending a signal');
  };
  const dlopen = process.dlopen;
  process.dlopen = function ghostDlopen(module, filename, ...rest) {
    if (addons.includes(packageOf(filename))) return dlopen.call(this, module, filename, ...rest);
    note('denied', `loading the native add-on ${String(filename)}`);
    throw refusal('Loading a native add-on');
  };
  // Node's internal bindings reach sockets and files without passing through anything sealed here.
  for (const name of ['binding', '_linkedBinding']) {
    process[name] = function ghostBinding(module) {
      note('denied', `Node's internal binding ${String(module)}`);
      throw refusal(`process.${name}`);
    };
  }
  os.setPriority = function ghostSetPriority() {
    note('denied', 'changing the priority of a process');
    throw refusal('Changing the priority of a process');
  };
  // WASI has its own access to files. Loaded only here, after worker.js has muted the ghost's output: loading it prints a warning.
  const wasi = process.getBuiltinModule('node:wasi');
  wasi.WASI = function ghostWASI() {
    note('denied', 'starting WASI, which has its own access to files');
    throw refusal('Starting WASI');
  };

  // Opening a file to write can already destroy it, so a ghost may open files only to read them.
  const { open, openSync } = fs;
  const promisesOpen = fs.promises.open;
  const opened = (target) => wrote(`FS open ${String(target)} -`);
  fs.openSync = function ghostOpenSync(target, flags, ...rest) {
    if (!opensToWrite(flags)) return openSync.call(this, target, flags, ...rest);
    opened(target);
    throw refusal('Opening a file to write');
  };
  fs.open = function ghostOpen(target, flags, ...rest) {
    if (typeof flags === 'function' || !opensToWrite(flags)) return open.call(this, target, flags, ...rest);
    opened(target);
    refuseLater(rest, 'Opening a file to write');
  };
  fs.promises.open = async function ghostPromisesOpen(target, flags, ...rest) {
    if (!opensToWrite(flags)) return promisesOpen.call(this, target, flags, ...rest);
    opened(target);
    throw refusal('Opening a file to write');
  };

  // With the overlay, these writes go to memory for the request (overlay.js); every other write is still refused.
  const OVERLAID = new Set(['writeFile', 'appendFile', 'unlink', 'rm', 'rmdir', 'rename', 'mkdir', 'copyFile', 'truncate']);
  for (const name of ['writeFile', 'appendFile', 'unlink', 'rm', 'rmdir', 'rename', 'mkdir', 'mkdtemp', 'copyFile', 'cp', 'truncate', 'symlink', 'link', 'chmod', 'chown', 'lchown', 'utimes', 'lutimes'].filter((n) => !(overlay && OVERLAID.has(n)))) {
    const effect = (target) => wrote(`FS ${name} ${String(target)} -`);
    fs[`${name}Sync`] = (target) => {
      effect(target);
      throw refusal(`fs.${name}`);
    };
    fs[name] = (target, ...rest) => {
      effect(target);
      refuseLater(rest, `fs.${name}`);
    };
    fs.promises[name] = async (target) => {
      effect(target);
      throw refusal(`fs.${name}`);
    };
  }
  if (overlay) installOverlay({ wrote });
  else {
    fs.createWriteStream = (target) => {
      wrote(`FS createWriteStream ${String(target)} -`);
      throw refusal('fs.createWriteStream');
    };
  }

  // A ghost opens files only to read them, so any other file descriptor it writes to belongs to the app's process. Only the ghost's own
  // output (1 and 2, which worker.js sends to the null device) is written.
  for (const name of ['write', 'writeSync', 'writev', 'writevSync']) {
    const inner = fs[name];
    fs[name] = function ghostFdWrite(fd, ...rest) {
      if (fd === 1 || fd === 2) return inner.call(this, fd, ...rest);
      note('denied', `writing to file descriptor ${fd}`);
      if (name.endsWith('Sync')) throw refusal('Writing to an open file');
      refuseLater(rest, 'Writing to an open file');
      return undefined;
    };
  }
  for (const name of ['ftruncate', 'fchmod', 'fchown', 'futimes']) {
    const effect = (fd) => wrote(`FS ${name} fd:${fd} -`);
    fs[`${name}Sync`] = (fd) => {
      effect(fd);
      throw refusal(`fs.${name}`);
    };
    fs[name] = (fd, ...rest) => {
      effect(fd);
      refuseLater(rest, `fs.${name}`);
    };
  }
  syncBuiltinESMExports();
}
