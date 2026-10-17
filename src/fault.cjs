'use strict';
const target = process.env.KERN_FAULT_FILE;
const source = process.env.KERN_FAULT_SOURCE;
const served = process.env.KERN_FAULT_SERVED;
const manifest = process.env.KERN_SERVE;
if (((target && source && served) || manifest) && !globalThis.__kernFault) {
  globalThis.__kernFault = true;
  const fs = require('fs');
  const path = require('path');
  const { fileURLToPath } = require('url');
  const sync = fs.readFileSync;
  const append = fs.appendFileSync;
  const bodies = new Map();
  const add = (file, from) => {
    let body = null;
    try { body = sync(from); } catch {}
    if (!body) return;
    bodies.set(path.resolve(file), body);
    try { bodies.set(fs.realpathSync(file), body); } catch {}
  };
  if (manifest) {
    let list = {};
    try { list = JSON.parse(sync(manifest, 'utf8')); } catch {}
    for (const [file, from] of Object.entries(list)) add(file, from);
  } else add(target, source);
  const find = (p) => {
    try {
      if (p == null || typeof p === 'number') return null;
      if (typeof p !== 'string' && !(p instanceof URL) && !Buffer.isBuffer(p)) return null;
      return bodies.get(path.resolve(p instanceof URL ? fileURLToPath(p) : String(p))) || null;
    } catch { return null; }
  };
  let noted = false;
  const give = (body, o) => {
    if (served && !noted) { noted = true; try { append(served, `${process.pid}\n`); } catch {} }
    const encoding = typeof o === 'string' ? o : o && typeof o === 'object' ? o.encoding : null;
    return encoding ? body.toString(encoding) : Buffer.from(body);
  };
  if (bodies.size) {
    fs.readFileSync = function readFileSync(p, o) { const b = find(p); return b ? give(b, o) : sync.apply(this, arguments); };
    const callback = fs.readFile;
    fs.readFile = function readFile(p, o, cb) {
      const b = find(p);
      if (!b) return callback.apply(this, arguments);
      const done = typeof o === 'function' ? o : cb;
      let value;
      try { value = give(b, typeof o === 'function' ? null : o); } catch (e) { process.nextTick(done, e); return; }
      process.nextTick(done, null, value);
    };
    const promised = fs.promises.readFile;
    fs.promises.readFile = function readFile(p, o) { const b = find(p); return b ? Promise.resolve().then(() => give(b, o)) : promised.apply(this, arguments); };
    require('module').syncBuiltinESMExports();
  }
}
