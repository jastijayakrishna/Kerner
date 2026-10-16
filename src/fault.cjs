'use strict';
const target = process.env.KERN_FAULT_FILE;
const source = process.env.KERN_FAULT_SOURCE;
const served = process.env.KERN_FAULT_SERVED;
if (target && source && served && !globalThis.__kernFault) {
  globalThis.__kernFault = true;
  const fs = require('fs');
  const path = require('path');
  const { fileURLToPath } = require('url');
  const sync = fs.readFileSync;
  const append = fs.appendFileSync;
  let body = null;
  try { body = sync(source); } catch {}
  const names = new Set([path.resolve(target)]);
  try { names.add(fs.realpathSync(target)); } catch {}
  const hit = (p) => {
    try {
      if (p == null || typeof p === 'number') return false;
      if (typeof p !== 'string' && !(p instanceof URL) && !Buffer.isBuffer(p)) return false;
      return names.has(path.resolve(p instanceof URL ? fileURLToPath(p) : String(p)));
    } catch { return false; }
  };
  let noted = false;
  const give = (o) => {
    if (!noted) { noted = true; try { append(served, `${process.pid}\n`); } catch {} }
    const encoding = typeof o === 'string' ? o : o && typeof o === 'object' ? o.encoding : null;
    return encoding ? body.toString(encoding) : Buffer.from(body);
  };
  if (body) {
    fs.readFileSync = function readFileSync(p, o) { return hit(p) ? give(o) : sync.apply(this, arguments); };
    const callback = fs.readFile;
    fs.readFile = function readFile(p, o, cb) {
      if (!hit(p)) return callback.apply(this, arguments);
      const done = typeof o === 'function' ? o : cb;
      let value;
      try { value = give(typeof o === 'function' ? null : o); } catch (e) { process.nextTick(done, e); return; }
      process.nextTick(done, null, value);
    };
    const promised = fs.promises.readFile;
    fs.promises.readFile = function readFile(p, o) { return hit(p) ? Promise.resolve().then(() => give(o)) : promised.apply(this, arguments); };
    require('module').syncBuiltinESMExports();
  }
}
