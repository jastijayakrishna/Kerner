'use strict';
const { isMainThread, Worker, parentPort, workerData } = require('worker_threads');
const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');
const MARK = '\n\u001ekern-probe\u001e\n';
const LATE = Symbol('late');
const stopping = [];

if (isMainThread) {
  const stdout = identity(1);
  main().then((r) => finish(r, stdout), (e) => finish({ version: 1, error: text(e, '') }, stdout));
} else {
  work();
}
function identity(fd) { try { const s = fs.fstatSync(fd); return `${s.dev}:${s.ino}`; } catch { return null; } }
function finish(report, stdout) {
  if (stdout === null || identity(1) !== stdout) process.exit(3);
  process.stdout.write(MARK + JSON.stringify(report) + '\n', () => process.exit(0));
}

async function main() {
  const plan = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
  if (plan.version !== 1) throw new Error('unsupported probe plan');
  const root = path.resolve(process.env.KERN_PROBE_ROOT || '/workspace');
  const started = Date.now();
  const reserve = plan.app ? Math.min(90000, Math.floor(plan.budget_ms / 3)) : 0;
  const deadline = started + plan.budget_ms - reserve;
  const modules = [];
  for (let m = 0; m < plan.modules.length; m++) {
    const left = plan.modules.length - m;
    const until = Math.min(deadline, Date.now() + Math.max(2000, Math.floor((deadline - Date.now()) / left)));
    modules.push(await probeModule(root, plan, plan.modules[m], until));
  }
  const app = plan.app ? await probeApp(root, plan.app, started + plan.budget_ms) : null;
  await Promise.all(stopping);
  return { version: 1, modules, app };
}

function probeModule(root, plan, mod, until) {
  const file = path.join(root, ...mod.path.split('/'));
  const result = { path: mod.path, load: null, ended: null, more: false, exports: {}, examples: [] };
  if (!fs.existsSync(file)) { result.load = 'missing'; result.ended = 'load'; return Promise.resolve(result); }
  if (Date.now() > until) { result.load = 'was not reached in time'; result.ended = 'time'; return Promise.resolve(result); }
  const prefix = mod.path + '#';
  const tuples = mod.tuples.slice(0, Math.max(1, plan.per_function || mod.tuples.length));
  const examples = plan.examples.map((e, i) => ({ ...e, i })).filter((e) => e.function.startsWith(prefix));
  const changes = plan.changes.filter((c) => c.function.startsWith(prefix) && c.when).map((c) => ({ name: c.function.slice(prefix.length), when: c.when }));
  const first = [...new Set(plan.changes.concat(plan.examples).filter((c) => c.function.startsWith(prefix)).map((c) => c.function.slice(prefix.length)))];
  const expected = new Map(examples.map((e) => [e.i, expectation(e, root)]));
  const asked = examples.map((e) => ({ i: e.i, function: e.function, args: e.args }));
  return new Promise((resolve) => {
    let keys = null, worker = null, timer = null, current = null, restarts = 0, done = false;
    const end = (why) => {
      if (done) return;
      done = true;
      result.ended = result.ended || why;
      clearTimeout(timer);
      if (worker) { const w = worker; worker = null; stopping.push(w.terminate().catch(() => {})); }
      for (const x of Object.values(result.exports)) {
        if (x.times) { const t = x.times.sort((a, b) => a - b); x.us = t.length ? t[Math.floor(t.length / 2)] : null; delete x.times; }
      }
      resolve(result);
    };
    const record = (c, out) => {
      if (c.phase === 'examples') result.examples.push({ i: c.i, out, ok: false });
      else { const x = result.exports[c.name]; if (x && x.calls && x.calls[c.k] === null) x.calls[c.k] = out; }
    };
    const after = (c) => (c.phase === 'examples' ? { phase: 'examples', pos: c.pos + 1 } : { phase: 'calls', name: c.name, k: c.k + 1 });
    const start = (from) => {
      if (done) return;
      if (Date.now() > until) return end('time');
      if (restarts > 30) return end('restarts');
      const key = require('crypto').randomBytes(16).toString('hex');
      const w = new Worker(__filename, { workerData: { key, file, root, seed: plan.seed, call_ms: plan.call_ms, tuples, examples: asked, first, from } });
      worker = w;
      const stopped = (out) => {
        if (worker !== w) return;
        worker = null;
        clearTimeout(timer);
        stopping.push(w.terminate().catch(() => {}));
        const c = current;
        current = null;
        if (!keys) {
          result.load = out === 't' ? 'did not finish loading in time' : 'stopped while loading';
          return end(out === 't' && Date.now() >= until ? 'time' : 'load');
        }
        if (!c) return end('reload');
        record(c, out);
        restarts++;
        start(after(c));
      };
      w.on('message', (m) => {
        if (worker !== w || !m || typeof m !== 'object' || m.key !== key) return;
        if (m.kind === 'load') {
          clearTimeout(timer);
          if (m.error) { if (!keys) result.load = String(m.error); return end(keys ? 'reload' : 'load'); }
          if (!keys) {
            keys = m.exports.map((x) => x.key);
            result.more = m.more === true;
            for (const x of m.exports) {
              const fn = x.type !== 'value';
              result.exports[x.key] = { type: x.type, value: x.value, calls: fn ? new Array(tuples.length).fill(null) : null, when: fn ? judge(changes, x.key, tuples) : null, us: null, times: fn ? [] : null };
            }
          }
          return;
        }
        if (m.kind === 'begin') {
          current = m.phase === 'examples' ? { phase: 'examples', pos: m.pos, i: m.i } : { phase: 'calls', name: m.name, k: m.k };
          clearTimeout(timer);
          timer = setTimeout(() => stopped('t'), plan.call_ms + 1000);
          return;
        }
        if (m.kind === 'example') {
          if (!current || current.phase !== 'examples' || current.i !== m.i) return;
          clearTimeout(timer);
          current = null;
          result.examples.push({ i: m.i, out: m.out, ok: holds(expected.get(m.i), m.out) });
          return;
        }
        if (m.kind === 'call') {
          if (!current || current.phase !== 'calls' || current.name !== m.name || current.k !== m.k) return;
          clearTimeout(timer);
          current = null;
          const x = result.exports[m.name];
          if (x && x.calls && x.calls[m.k] === null) { x.calls[m.k] = m.out; if (m.out[0] === 'r') x.times.push(m.us); }
          if (Date.now() > until) end('time');
          return;
        }
        if (m.kind === 'done') return end('done');
        if (m.kind === 'fatal') { if (!keys) result.load = String(m.error); end(keys ? 'reload' : 'load'); }
      });
      w.on('error', () => stopped('x'));
      w.on('exit', () => stopped('x'));
      timer = setTimeout(() => stopped('t'), Math.max(1000, Math.min(plan.load_ms || 20000, until - Date.now())));
    };
    start({ phase: 'examples', pos: 0 });
  });
}

function judge(changes, name, tuples) {
  const tests = changes.filter((c) => c.name === '*' || c.name === name);
  if (!tests.length) return null;
  const vm = require('vm');
  return tuples.map((tuple) => tests.some((c) => { try { return !!vm.runInThisContext('(' + c.when + ')(...(' + tuple + '))', { timeout: 250 }); } catch { return false; } }));
}
function expectation(e, root) {
  if (e.throws !== null && e.throws !== undefined) return { throws: e.throws };
  try { return { returns: 'r ' + ser(require('vm').runInThisContext('(' + e.returns + ')', { timeout: 250 }), root) }; } catch { return { returns: null }; }
}
function holds(x, out) {
  if (!x || typeof out !== 'string') return false;
  if (x.throws !== undefined) return out.startsWith('e ') && (x.throws === true || out.slice(2).startsWith(x.throws + ':'));
  return x.returns !== null && out === x.returns;
}

function work() {
  const d = workerData;
  const key = d.key;
  delete d.key;
  const send = parentPort.postMessage.bind(parentPort);
  const tools = { evaluate: eval, apply: Reflect.apply, construct: Reflect.construct, now: process.hrtime.bigint, isArray: Array.isArray, Promise, setTimeout, clearTimeout };
  const pre = /--require[= ]"?([^\s"]*monitor\.cjs)/.exec(process.env.NODE_OPTIONS || '');
  if (pre) { try { require(pre[1]); } catch {} }
  steady(d.seed);
  process.on('uncaughtException', () => {});
  process.on('unhandledRejection', () => {});
  run(d, key, send, tools).catch((e) => send({ key, kind: 'fatal', error: text(e, d.root) }));
}
async function run(d, key, send, t) {
  let ns;
  try { ns = await import(pathToFileURL(d.file).href); }
  catch (e) { return send({ key, kind: 'load', error: text(e, d.root) }); }
  const list = exportsOf(ns);
  send({ key, kind: 'load', exports: list.slice(0, 2000).map((x) => ({ key: x.key, type: x.type, value: x.type === 'value' ? ser(x.value, d.root) : null })), more: list.length > 2000 });
  const from = d.from;
  if (from.phase === 'examples') {
    for (let pos = from.pos; pos < d.examples.length; pos++) {
      const e = d.examples[pos];
      send({ key, kind: 'begin', phase: 'examples', pos, i: e.i });
      const name = e.function.slice(e.function.indexOf('#') + 1);
      const x = list.find((y) => y.key === name);
      const out = x && x.type !== 'value' ? await call(x, args(e.args, t), d.call_ms, d.root, t) : 'm';
      send({ key, kind: 'example', i: e.i, out });
    }
  }
  const fns = list.filter((x) => x.type !== 'value');
  const order = fns.filter((x) => d.first.includes(x.key)).concat(fns.filter((x) => !d.first.includes(x.key))).slice(0, 200);
  const at = from.phase === 'calls' ? order.findIndex((x) => x.key === from.name) : 0;
  for (let f = at < 0 ? order.length : at; f < order.length; f++) {
    const x = order[f];
    for (let k = f === at && from.phase === 'calls' ? from.k : 0; k < d.tuples.length; k++) {
      send({ key, kind: 'begin', phase: 'calls', name: x.key, k });
      const begin = t.now();
      const out = await call(x, args(d.tuples[k], t), d.call_ms, d.root, t);
      const us = Number((t.now() - begin) / 1000n);
      send({ key, kind: 'call', name: x.key, k, out, us });
    }
  }
  send({ key, kind: 'done' });
}

function exportsOf(ns) {
  const list = [];
  const entry = (key, v, self) => {
    if (typeof v !== 'function') return { key, type: 'value', value: v };
    let source = '';
    try { source = Function.prototype.toString.call(v); } catch {}
    return { key, type: /^class\b/.test(source) ? 'class' : 'function', fn: v, self };
  };
  const names = Object.keys(ns).filter((k) => k !== 'default' && k !== 'module.exports').sort();
  for (const k of names) { let v; try { v = ns[k]; } catch { continue; } list.push(entry(k, v, ns)); }
  let d;
  try { d = ns.default; } catch {}
  if (typeof d === 'function') list.push(entry('default', d, ns));
  else if (d && typeof d === 'object' && !Array.isArray(d)) {
    for (const k of Object.keys(d).sort()) {
      let v;
      try { v = d[k]; } catch { continue; }
      if (typeof v !== 'function') continue;
      if (!names.includes(k)) { list.push(entry(k, v, d)); continue; }
      let same = false;
      try { same = ns[k] === v; } catch {}
      if (!same) list.push(entry('default.' + k, v, d));
    }
  } else if ('default' in ns && !names.length) list.push(entry('default', d, ns));
  return list;
}
function args(src, t) {
  try { const v = t.evaluate(src); return t.isArray(v) ? v : [v]; } catch { return null; }
}
async function call(x, a, ms, root, t) {
  if (a === null) return 'i';
  let timer = null;
  try {
    let r = x.type === 'class' ? t.construct(x.fn, a) : t.apply(x.fn, x.self, a);
    let then = null;
    try { then = r !== null && (typeof r === 'object' || typeof r === 'function') ? r.then : null; } catch {}
    if (typeof then === 'function') {
      const promise = r;
      r = await new t.Promise((ok, no) => {
        timer = t.setTimeout(() => no(LATE), ms);
        try { t.apply(then, promise, [ok, no]); } catch (e) { no(e); }
      });
    }
    return 'r ' + ser(r, root);
  } catch (e) {
    return e === LATE ? 't' : 'e ' + text(e, root);
  } finally {
    if (timer) t.clearTimeout(timer);
  }
}

function steady(seed) {
  let s = parseInt(String(seed).slice(0, 8), 16) >>> 0;
  const rand = () => { s = (s + 0x6d2b79f5) >>> 0; let t = s; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
  Math.random = rand;
  const Real = Date;
  const NOW = Real.UTC(2024, 0, 1, 12, 0, 0);
  function Fixed(...a) { if (!new.target) return new Real(NOW).toString(); return Reflect.construct(Real, a.length ? a : [NOW], new.target); }
  Fixed.prototype = Real.prototype;
  Fixed.now = () => NOW;
  Fixed.UTC = Real.UTC;
  Fixed.parse = Real.parse;
  globalThis.Date = Fixed;
  const crypto = require('crypto');
  const bytes = (n) => { const b = Buffer.alloc(n); for (let i = 0; i < n; i++) b[i] = Math.floor(rand() * 256); return b; };
  const fill = (a) => { const b = bytes(a.byteLength); new Uint8Array(a.buffer, a.byteOffset, a.byteLength).set(b); return a; };
  const uuid = () => { const b = bytes(16); b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80; const h = b.toString('hex'); return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`; };
  const put = (o, name, value) => { try { Object.defineProperty(o, name, { value, writable: true, configurable: true, enumerable: true }); } catch { try { o[name] = value; } catch {} } };
  put(crypto, 'randomBytes', (n, cb) => { const b = bytes(n); if (typeof cb === 'function') { process.nextTick(cb, null, b); return undefined; } return b; });
  put(crypto, 'randomUUID', uuid);
  put(crypto, 'getRandomValues', fill);
  put(crypto, 'randomInt', (a, b, cb) => { const [lo, hi] = b === undefined || typeof b === 'function' ? [0, a] : [a, b]; const v = lo + Math.floor(rand() * (hi - lo)); const done = typeof b === 'function' ? b : cb; if (typeof done === 'function') { process.nextTick(done, null, v); return undefined; } return v; });
  if (globalThis.crypto) { put(globalThis.crypto, 'getRandomValues', fill); put(globalThis.crypto, 'randomUUID', uuid); }
  try { require('module').syncBuiltinESMExports(); } catch {}
}

function digest() { return require('crypto').createHash('sha256'); }
function unrooted(s, root) { return root ? s.split(pathToFileURL(root).href).join('').split(root).join('') : s; }
function text(e, root) {
  let name = typeof e, msg = '';
  try {
    if (e && (typeof e === 'object' || typeof e === 'function')) { name = String(e.name || (e.constructor && e.constructor.name) || 'Error'); msg = String(e.message !== undefined ? e.message : e); }
    else msg = String(e);
  } catch { msg = '?'; }
  const s = unrooted(name + ': ' + msg, root).replace(/\r?\n/g, '\\n');
  return s.length > 300 ? s.slice(0, 280) + '...#' + digest().update(s).digest('hex').slice(0, 16) : s;
}
function ser(v, root) {
  const shown = write(v, root, false);
  if (!shown.cut && shown.text.length <= 240) return shown.text;
  const h = digest();
  write(v, root, true, (s) => h.update(s));
  return shown.text.slice(0, 120) + '...#' + h.digest('hex').slice(0, 16);
}
function write(v, root, full, sink) {
  const state = { text: '', cut: false, size: 0 };
  const FULL = 64 * 1024 * 1024;
  const STOP = Symbol('stop');
  const out = (s) => {
    if (sink) { state.size += s.length; if (state.size > FULL) throw STOP; sink(s); return; }
    if (state.text.length < 4096) state.text += s; else state.cut = true;
  };
  const most = (n) => (full ? Infinity : n);
  const seen = new Set();
  const go = (x, depth) => {
    if (x === undefined) return out('undefined');
    if (x === null) return out('null');
    switch (typeof x) {
      case 'number': return out(Object.is(x, -0) ? '-0' : String(x));
      case 'bigint': return out(x + 'n');
      case 'boolean': return out(String(x));
      case 'string': { let s = unrooted(x, root); if (s.length > most(1000)) { s = s.slice(0, 1000); state.cut = true; } return out(JSON.stringify(s)); }
      case 'symbol': return out('Symbol(' + JSON.stringify(String(x.description || '')) + ')');
      case 'function': return out('[Function ' + (x.name || 'anonymous') + ']');
    }
    if (seen.has(x)) return out('[Circular]');
    if (depth > (full ? 64 : 6)) { state.cut = true; return out('[...]'); }
    seen.add(x);
    try {
      if (x instanceof Date) return out('Date(' + (isNaN(x.getTime()) ? 'Invalid' : x.toISOString()) + ')');
      if (x instanceof RegExp) return out(String(x));
      if (x instanceof Error) return out('Error(' + text(x, root) + ')');
      if (Buffer.isBuffer(x)) {
        const n = Math.min(x.length, most(64));
        if (n < x.length) state.cut = true;
        return out('Buffer(' + x.length + ':' + x.subarray(0, n).toString('hex') + ')');
      }
      if (ArrayBuffer.isView(x) && typeof x.length === 'number') {
        const n = Math.min(x.length, most(64));
        if (n < x.length) state.cut = true;
        return out((x.constructor && x.constructor.name) + '(' + Array.from(x.slice(0, n)).join(',') + (n < x.length ? ',...' : '') + ')');
      }
      if (x instanceof Map || x instanceof Set) {
        const items = [...x];
        const n = Math.min(items.length, most(100));
        if (n < items.length) state.cut = true;
        out(x instanceof Map ? 'Map{' : 'Set{');
        for (let i = 0; i < n; i++) {
          if (i) out(',');
          if (x instanceof Map) { go(items[i][0], depth + 1); out('=>'); go(items[i][1], depth + 1); } else go(items[i], depth + 1);
        }
        return out('}');
      }
      let then = null;
      try { then = x.then; } catch {}
      if (typeof then === 'function') return out('Promise');
      if (Array.isArray(x)) {
        const n = Math.min(x.length, most(200));
        out('[');
        for (let i = 0; i < n; i++) { if (i) out(','); go(x[i], depth + 1); }
        if (n < x.length) { state.cut = true; out(',...' + x.length); }
        return out(']');
      }
      const proto = Object.getPrototypeOf(x);
      const name = proto === null ? 'null' : proto === Object.prototype ? '' : (proto.constructor && proto.constructor.name) || '?';
      const keys = Object.keys(x).sort();
      const n = Math.min(keys.length, most(200));
      if (n < keys.length) state.cut = true;
      out(name + '{');
      for (let i = 0; i < n; i++) {
        if (i) out(',');
        out(JSON.stringify(keys[i]) + ':');
        let value;
        try { value = x[keys[i]]; } catch { out('[Throws]'); continue; }
        go(value, depth + 1);
      }
      return out('}');
    } finally {
      seen.delete(x);
    }
  };
  try { go(v, 0); } catch (e) { if (e === STOP) { if (sink) sink('[too large]'); } else { state.text = '[Unreadable]'; state.cut = false; } }
  return state;
}

async function probeApp(root, app, deadline) {
  const { spawn, spawnSync } = require('child_process');
  const env = Object.assign({}, process.env, app.env, { PORT: String(app.port) });
  if (app.setup) {
    const r = spawnSync(app.setup[0], app.setup.slice(1), { cwd: root, env, stdio: 'ignore', timeout: Math.max(1000, Math.min(120000, deadline - Date.now())) });
    if (r.status !== 0) return { ready: false, error: 'the setup command failed', results: {} };
  }
  const posix = process.platform !== 'win32';
  const child = spawn(app.start[0], app.start.slice(1), { cwd: root, env, stdio: 'ignore', detached: posix });
  child.on('error', () => {});
  try {
    const wait = Math.min(deadline, Date.now() + 60000);
    let ready = false;
    while (Date.now() < wait) {
      const s = await ask(app.port, 'GET ' + app.ready, {});
      if (/^\d/.test(s) && Number(s.split(' ')[0]) < 500) { ready = true; break; }
      if (child.exitCode !== null) break;
      await new Promise((r) => setTimeout(r, 200));
    }
    if (!ready) return { ready: false, error: 'the app did not answer ' + app.ready, results: {} };
    const results = {};
    for (const route of app.routes) {
      for (const role of Object.keys(app.roles).sort()) {
        if (Date.now() > deadline) break;
        results[route + ' as ' + role] = await ask(app.port, route, app.roles[role]);
      }
    }
    return { ready: true, error: null, results };
  } finally {
    try { if (posix) process.kill(-child.pid, 'SIGKILL'); } catch {}
    try { child.kill('SIGKILL'); } catch {}
  }
}
function ask(port, route, headers) {
  const http = require('http');
  const space = route.indexOf(' ');
  return new Promise((done) => {
    let limit = null;
    const resolve = (s) => { clearTimeout(limit); done(s); };
    const req = http.request({ host: '127.0.0.1', port, method: route.slice(0, space), path: route.slice(space + 1), headers, agent: false, timeout: 5000 }, (res) => {
      let where = '';
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) { try { where = ' ' + new URL(res.headers.location, 'http://app').pathname; } catch {} }
      const status = String(res.statusCode) + where;
      res.on('error', () => resolve(status));
      res.on('end', () => resolve(status));
      res.resume();
    });
    limit = setTimeout(() => { req.destroy(); resolve('timeout'); }, 10000);
    req.on('timeout', () => { req.destroy(); resolve('timeout'); });
    req.on('error', (e) => resolve('error ' + (e.code || 'unknown')));
    req.end();
  });
}
