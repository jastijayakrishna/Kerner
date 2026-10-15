import net from 'node:net';
import dgram from 'node:dgram';
import dns from 'node:dns';
import fs from 'node:fs';
import os from 'node:os';
import childProcess from 'node:child_process';
import workerThreads from 'node:worker_threads';
import { syncBuiltinESMExports } from 'node:module';
import { callKey, isSession, isWrite, readAllowed, shapeOf } from './keys.js';
import { installOverlay, resetOverlay } from './overlay.js';
import { mine, never } from './turn.js';

let job = null;
const session = new Map();
const MOST_SESSION = 1000;
const MOST_WRITES = 10000;

export function beginJob(io) {
  resetOverlay();
  for (const call of io ?? []) {
    if (!isSession(call.key)) continue;
    session.delete(call.key);
    session.set(call.key, call);
    if (session.size > MOST_SESSION) session.delete(session.keys().next().value);
  }
  job = { calls: (io ?? []).map((call) => ({ call, shape: shapeOf(call.key), used: false })), effects: [], writes: new Map(), tooMany: false, events: [], weighty: 0 };
}

export function endJob() {
  const out = job ? { effects: job.effects, writes: [...job.writes], tooMany: job.tooMany, events: job.events } : { effects: [], writes: [], tooMany: false, events: [] };
  job = null;
  return out;
}

export function take(key) {
  if (!job || !mine()) return undefined;
  const shape = shapeOf(key);
  if (isSession(key)) {
    const here = job.calls.find((entry) => entry.call.key === key) ?? job.calls.find((entry) => entry.shape === shape);
    if (here) return here.call;
    return session.get(key) ?? [...session.values()].findLast((call) => shapeOf(call.key) === shape);
  }
  const found = job.calls.find((entry) => !entry.used && entry.call.key === key) ?? job.calls.find((entry) => !entry.used && entry.shape === shape);
  if (!found) return undefined;
  found.used = true;
  if (found.call.key !== key) note('loose', found.call.key);
  return found.call;
}

const WEIGHTY = new Set(['denied', 'unmatched-read', 'unmatched-write']);

export function note(type, detail) {
  if (!mine() || !job) return;
  const weighty = WEIGHTY.has(type);
  if (weighty ? job.weighty >= 200 : job.events.length - job.weighty >= 50) return;
  if (weighty) job.weighty++;
  job.events.push({ type, detail: String(detail).slice(0, 300) });
}

export function wrote(effect) {
  if (!mine() || !job) return;
  if (job.effects.length < 200) job.effects.push(effect);
  if (job.writes.has(effect) || job.writes.size < MOST_WRITES) job.writes.set(effect, (job.writes.get(effect) ?? 0) + 1);
  else job.tooMany = true;
}

const NULL_BODY = new Set([101, 204, 205, 304]);
const loopback = (host) => ['127.0.0.1', 'localhost', '::1', '[::1]'].includes(String(host));
const refusal = (what) => Object.assign(new Error(`${what} is refused in a Kern ghost run`), { code: 'EACCES' });
const { O_WRONLY, O_RDWR, O_CREAT, O_TRUNC, O_APPEND } = fs.constants;
const opensToWrite = (flags) => (typeof flags === 'number' ? (flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC | O_APPEND)) !== 0 : !/^(r|rs|sr)$/.test(String(flags ?? 'r')));
function refuseLater(args, what) {
  const done = args.findLast((a) => typeof a === 'function');
  if (done) process.nextTick(done, refusal(what));
}
export const packageOf = (file) => String(file).replaceAll('\\', '/').match(/.*node_modules\/((?:@[^/]+\/)?[^/]+)\//)?.[1] ?? null;

export const addonName = (file) => packageOf(file) ?? String(file).replaceAll('\\', '/').split('/').pop().split('.')[0];
const LOOKUPS = ['lookup', 'lookupService', 'resolve', 'resolve4', 'resolve6', 'resolveAny', 'resolveCaa', 'resolveCname', 'resolveMx', 'resolveNaptr', 'resolveNs', 'resolvePtr', 'resolveSoa', 'resolveSrv', 'resolveTxt', 'reverse'];

export function installGhostIO({ harnessPort, ports = [harnessPort], addons = [], realReads = [], ask = null, overlay = false }) {
  const allowed = new Set(ports.filter((port) => Number.isInteger(port)));
  const realFetch = globalThis.fetch;
  globalThis.fetch = async function ghostFetch(input, init) {
    const request = new Request(input, init);
    const url = new URL(request.url);
    if (loopback(url.hostname) && allowed.has(Number(url.port))) return realFetch(request);
    if (!mine()) return never();
    const body = request.body ? Buffer.from(await request.arrayBuffer()) : Buffer.alloc(0);
    const key = callKey(request.method, request.url, body);
    const write = isWrite(request.method);
    if (write) wrote(key);
    let recorded = take(key);
    if (!recorded && !write && ask && readAllowed(realReads, request.method, request.url)) {
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
    const headers = Object.entries(recorded.headers ?? {})
      .filter(([name]) => !['content-encoding', 'content-length', 'transfer-encoding'].includes(name))
      .flatMap(([name, value]) => (Array.isArray(value) ? value.map((one) => [name, one]) : [[name, value]]));
    const answer = NULL_BODY.has(recorded.status) || recorded.body == null ? null : recorded.body;
    return new Response(answer, { status: recorded.status, headers });
  };

  const realConnect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function ghostConnect(...args) {
    const first = Array.isArray(args[0]) ? args[0][0] : args[0];
    const options = first !== null && typeof first === 'object' ? first : { port: first, host: typeof args[1] === 'string' ? args[1] : 'localhost' };
    if (options.path == null && loopback(options.host ?? 'localhost') && allowed.has(Number(options.port))) return realConnect.apply(this, args);
    note('denied', `a network connection to ${options.path ?? `${options.host ?? 'localhost'}:${options.port}`}`);
    process.nextTick(() => this.destroy(refusal('A network connection')));
    return this;
  };

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

  workerThreads.Worker = function ghostWorker() {
    note('denied', 'starting a worker thread');
    throw refusal('Starting a worker thread');
  };

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

  process.kill = function ghostKill(pid, signal) {
    note('denied', `sending ${signal ?? 'SIGTERM'} to process ${pid}`);
    throw refusal('Sending a signal');
  };
  const dlopen = process.dlopen;
  process.dlopen = function ghostDlopen(module, filename, ...rest) {
    if (addons.includes(addonName(filename))) return dlopen.call(this, module, filename, ...rest);
    note('denied', `loading the native add-on ${String(filename)}`);
    throw refusal('Loading a native add-on');
  };
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
  const wasi = process.getBuiltinModule('node:wasi');
  wasi.WASI = function ghostWASI() {
    note('denied', 'starting WASI, which has its own access to files');
    throw refusal('Starting WASI');
  };

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
