'use strict';
// Kern's behaviour recorder, preloaded into every Node process of a check, a test run or a probe: the hosts the code reaches (and how often),
// the names it looks up, the programs it starts, the variables it reads, and the files of the repository it deletes or overwrites.
const out = process.env.KERN_MONITOR;
// The main thread of Kern's own probe run is the harness, never code under test: starting the app and handing it the environment are not
// the app's behaviour. Code under test runs in the probe's workers and in the app, which are recorded. Only Kern's control mount holds
// these two files, so repository code cannot claim to be the harness.
const harness = (() => { try { return require('worker_threads').isMainThread && process.argv[1] === '/kern/probe.cjs' && process.argv[2] === '/kern/probe.json'; } catch { return false; } })();
if (out && !harness && !globalThis.__kernMonitor) {
  globalThis.__kernMonitor = true;
  const fs = require('fs');
  const path = require('path');
  // The recorder's own log never goes through the functions it watches.
  const append = fs.appendFileSync;
  const lstat = fs.lstatSync;
  const seen = new Set();
  const note = (kind, value) => {
    const line = `${kind}:${String(value).replace(/[\r\n]/g, ' ').slice(0, 200)}`;
    if (seen.has(line)) return;
    seen.add(line);
    try { append(out, `${line}\n`); } catch {}
  };
  // Every attempt to reach a host is counted as well, so Kern can compare how often the old and the new code call it (a paid API, say).
  let counted = 0;
  const count = (key) => { if (counted++ < 5000) { try { append(out, `count ${key}\n`); } catch {} } };
  const loopback = (h) => h == null || h === '' || /^(localhost|127(\.\d+){3}|::1|0\.0\.0\.0|::)$/i.test(String(h).replace(/^\[|\]$/g, ''));
  const target = (host, port) => (loopback(host) ? 'loopback' : `${String(host).toLowerCase()}:${port ?? ''}`);
  const program = (file) => String(file || '').trim().split(/\s+/)[0].split(/[\\/]/).pop().replace(/\.exe$/i, '');
  const net = require('net');
  const connect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function (...args) {
    try {
      const a = Array.isArray(args[0]) ? args[0][0] : args[0];
      let t = null;
      if (a && typeof a === 'object') t = a.path ? 'ipc' : target(a.host, a.port);
      else if (typeof a === 'number' || /^\d+$/.test(String(a))) t = target(typeof args[1] === 'string' ? args[1] : 'localhost', a);
      else if (typeof a === 'string') t = 'ipc';
      if (t) { note('net', t); if (t !== 'ipc' && t !== 'loopback') count(`net:${t}`); }
    } catch {}
    return connect.apply(this, args);
  };
  const dgram = require('dgram');
  const send = dgram.Socket.prototype.send;
  dgram.Socket.prototype.send = function (...args) { note('net', 'udp'); return send.apply(this, args); };
  const dns = require('dns');
  const wrapDns = (owner) => {
    for (let o = owner; o && o !== Object.prototype && o !== Function.prototype; o = Object.getPrototypeOf(o)) {
      for (const name of Object.getOwnPropertyNames(o)) {
        const f = Object.getOwnPropertyDescriptor(o, name).value;
        if (typeof f !== 'function' || !/^(lookup|lookupService|reverse|resolve\w*)$/.test(name) || f.__kern) continue;
        const wrapped = function (host, ...rest) { if (typeof host === 'string' && !loopback(host)) note('dns', host.toLowerCase()); return f.call(this, host, ...rest); };
        wrapped.__kern = true;
        o[name] = wrapped;
      }
    }
  };
  for (const owner of [dns, dns.promises, dns.Resolver && dns.Resolver.prototype, dns.promises && dns.promises.Resolver && dns.promises.Resolver.prototype]) wrapDns(owner);
  const cp = require('child_process');
  for (const name of ['spawn', 'spawnSync', 'execFile', 'execFileSync', 'exec', 'execSync', 'fork']) {
    const f = cp[name];
    cp[name] = function (file, ...rest) { note('exec', name === 'fork' ? 'node' : program(file)); return f.call(this, file, ...rest); };
  }
  // Files. A path counts when it belongs to the repository the run started from (the sandbox keeps that snapshot read-only at /input) or lies
  // outside the workspace and the temporary folder. Files the run made itself, the dependency install and temporary files never count.
  const ws = path.resolve(process.env.KERN_WORKSPACE || '/workspace');
  const input = path.resolve(process.env.KERN_INPUT || '/input');
  const tmp = path.resolve(require('os').tmpdir());
  const exists = (p) => { try { lstat(p); return true; } catch { return false; } };
  const snapshot = exists(input);
  const within = (root, abs) => { const r = path.relative(root, abs); return r === '' ? '.' : !r.startsWith('..') && !path.isAbsolute(r) ? r.split(path.sep).join('/') : null; };
  const made = new Set();
  const where = (p) => {
    try {
      if (p == null || typeof p === 'number') return null;
      return path.resolve(p instanceof URL ? require('url').fileURLToPath(p) : Buffer.isBuffer(p) ? p.toString() : String(p));
    } catch { return null; }
  };
  const owned = (abs) => {
    if (!abs || made.has(abs)) return null;
    const rel = within(ws, abs);
    if (rel !== null) {
      if (rel === 'node_modules' || rel.startsWith('node_modules/')) return null;
      return (snapshot ? exists(path.join(input, rel)) : exists(abs)) ? rel : null;
    }
    if (within(tmp, abs) !== null || abs.startsWith('/proc/') || abs.startsWith('/dev/')) return null;
    return abs.split(path.sep).join('/');
  };
  const destroy = (kind, p) => { const abs = where(p); const shown = owned(abs); if (shown !== null && exists(abs)) note(kind, shown); };
  const write = (p) => { const abs = where(p); if (!abs) return; if (exists(abs)) destroy('overwrite', abs); else made.add(abs); };
  const create = (p) => { const abs = where(p); if (abs && !exists(abs)) made.add(abs); };
  const truncating = (flags) => (typeof flags === 'string' ? flags.includes('w') : typeof flags === 'number' && (flags & fs.constants.O_TRUNC) !== 0);
  const creating = (flags) => (typeof flags === 'string' ? /[awx]/.test(flags) : typeof flags === 'number' && (flags & fs.constants.O_CREAT) !== 0);
  const wrap = (owner, name, before) => {
    const f = owner && owner[name];
    if (typeof f !== 'function' || f.__kern) return;
    const g = function (...a) { try { before(...a); } catch {} return f.apply(this, a); };
    g.__kern = true;
    try { Object.defineProperty(g, 'name', { value: f.name }); Object.defineProperty(g, 'length', { value: f.length }); } catch {}
    owner[name] = g;
  };
  for (const owner of [fs, fs.promises]) {
    for (const n of ['unlink', 'unlinkSync', 'rm', 'rmSync', 'rmdir', 'rmdirSync']) wrap(owner, n, (p) => destroy('delete', p));
    for (const n of ['rename', 'renameSync']) wrap(owner, n, (from, to) => { destroy('delete', from); write(to); });
    // appendFile reaches writeFile with the flag 'a': adding to a file is not destroying it.
    for (const n of ['writeFile', 'writeFileSync']) wrap(owner, n, (p, data, o) => { if (truncating((o && typeof o === 'object' && o.flag) || 'w')) write(p); else create(p); });
    for (const n of ['truncate', 'truncateSync']) wrap(owner, n, (p) => destroy('overwrite', p));
    for (const n of ['copyFile', 'copyFileSync', 'cp', 'cpSync']) wrap(owner, n, (from, to) => write(to));
    for (const n of ['appendFile', 'appendFileSync', 'mkdir', 'mkdirSync']) wrap(owner, n, (p) => create(p));
    for (const n of ['open', 'openSync']) wrap(owner, n, (p, flags) => { if (truncating(flags)) write(p); else if (creating(flags)) create(p); });
  }
  wrap(fs, 'createWriteStream', (p, o) => { const flags = (o && typeof o === 'object' && o.flags) || 'w'; if (truncating(flags)) write(p); else create(p); });
  require('module').syncBuiltinESMExports();
  const env = process.env;
  process.env = new Proxy(env, {
    get(t, k) { if (typeof k === 'string') note('env', k); return Reflect.get(t, k); },
    has(t, k) { if (typeof k === 'string') note('env', k); return Reflect.has(t, k); },
    ownKeys(t) { note('env', '*'); return Reflect.ownKeys(t); },
    set(t, k, v) { return Reflect.set(t, k, v); },
    deleteProperty(t, k) { return Reflect.deleteProperty(t, k); },
  });
}
