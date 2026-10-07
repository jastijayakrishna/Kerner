'use strict';
const out = process.env.KERN_MONITOR;
if (out) {
  const fs = require('fs');
  const seen = new Set();
  const note = (kind, value) => {
    const line = `${kind}:${String(value).replace(/[\r\n]/g, ' ').slice(0, 200)}`;
    if (seen.has(line)) return;
    seen.add(line);
    try { fs.appendFileSync(out, `${line}\n`); } catch {}
  };
  const loopback = (h) => h == null || h === '' || /^(localhost|127(\.\d+){3}|::1|0\.0\.0\.0|::)$/i.test(String(h).replace(/^\[|\]$/g, ''));
  const target = (host, port) => (loopback(host) ? 'loopback' : `${String(host).toLowerCase()}:${port ?? ''}`);
  const program = (file) => String(file || '').trim().split(/\s+/)[0].split(/[\\/]/).pop().replace(/\.exe$/i, '');
  const net = require('net');
  const connect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function (...args) {
    try {
      const a = Array.isArray(args[0]) ? args[0][0] : args[0];
      if (a && typeof a === 'object') note('net', a.path ? 'ipc' : target(a.host, a.port));
      else if (typeof a === 'number' || /^\d+$/.test(String(a))) note('net', target(typeof args[1] === 'string' ? args[1] : 'localhost', a));
      else if (typeof a === 'string') note('net', 'ipc');
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
