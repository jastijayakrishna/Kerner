import { register, createRequire } from 'node:module';
import { pathToFileURL, fileURLToPath } from 'node:url';
import fs from 'node:fs';
import { ownFiles, relativeFile, blankMaps, ASSET, MARKER, assetExports } from './drive-support.mjs';

const config = JSON.parse(fs.readFileSync('/kern/drive.json', 'utf8'));
const LIMIT = 100;
await import('/kern-tools/node_modules/tsx/dist/loader.mjs');

const readRaw = fs.readFileSync;
{
  const SOURCE = /\.[cm]?[jt]sx?$/;
  fs.readFileSync = function (file, ...rest) {
    const out = readRaw.call(this, file, ...rest);
    const p = typeof file === 'string' ? file : file instanceof URL ? fileURLToPath(file) : '';
    if (!p.startsWith('/workspace/') || p.includes('/node_modules/') || !SOURCE.test(p)) return out;
    if (typeof out === 'string') return blankMaps(out);
    return Buffer.isBuffer(out) && out.includes('sourceMappingURL') ? Buffer.from(blankMaps(out.toString('latin1')), 'latin1') : out;
  };
  createRequire(import.meta.url)('node:module').syncBuiltinESMExports();
}

const cjs = new Map(config.modules.filter((m) => m.cjs).map((m) => ['/workspace/' + m.path, m]));

const esm = config.modules.filter((m) => !m.cjs && m.functions.length).map((m) => {
  const url = pathToFileURL('/workspace/' + m.path).href;
  return { url, wrap: 'kern-wrap:' + url, path: m.path, functions: m.functions, hasDefault: m.default === true };
});
const stubs = Object.fromEntries((config.stubs || []).map((s) => [s, pathToFileURL('/kern/stubs/' + s + '.mjs').href]));
register('./drive-hooks.mjs', import.meta.url, { data: { esm, stubs } });
{
  const Module = createRequire(import.meta.url)('node:module');
  for (const ext of ['.css', '.scss', '.sass', '.less', '.styl', '.pcss', '.svg', '.png', '.jpg', '.jpeg', '.gif', '.webp', '.avif', '.ico', '.bmp', '.woff', '.woff2',
    '.ttf', '.otf', '.eot', '.mp3', '.mp4', '.webm', '.wav', '.ogg', '.pdf', '.md', '.mdx', '.txt', '.html', '.glsl', '.frag', '.vert', '.wasm', '.sql']) {
    Module._extensions[ext] = (m, filename) => { m.exports = assetExports(filename); };
  }
  const EMPTY = '/kern/empty-marker.js';
  const files = new Map((config.stubs || []).map((s) => [s, '/kern/stubs/' + s + '.mjs']));
  const own = new Set(files.values());
  const resolveFilename = Module._resolveFilename;
  Module._resolveFilename = function (request, parent, ...rest) {
    if (MARKER.test(request)) return EMPTY;
    const stub = files.get(request);
    if (stub && !(parent && own.has(parent.filename))) {
      let real = null;
      try { real = resolveFilename.call(this, request, parent, ...rest); } catch {}
      for (const file of real === null ? ownFiles(request, parent && parent.filename) : []) {
        try { real = resolveFilename.call(this, file, parent, ...rest); break; } catch {}
      }
      return real === null || real.includes('/node_modules/') ? stub : real;
    }
    try { return resolveFilename.call(this, request, parent, ...rest); }
    catch (e) {
      for (const file of ownFiles(request, parent && parent.filename)) {
        try { return resolveFilename.call(this, file, parent, ...rest); } catch {}
      }
      if (ASSET.test(request)) return '/kern/assets/' + request.replace(/[?#].*$/, '').replace(/^(\.\.?\/)+/, '');
      throw e;
    }
  };
}

{
  const Module = createRequire(import.meta.url)('node:module');
  const esbuild = createRequire(import.meta.url)('/kern-tools/node_modules/esbuild');
  const OFF = /JSX syntax extension is not currently enabled/;
  const TAG = /<\/|\/>|<>/;
  const hasJsx = (code) => { if (!TAG.test(code)) return false; try { esbuild.transformSync(code, { loader: 'js' }); return false; } catch (e) { return OFF.test(String(e && e.message)); } };
  const compile = (file, code, format) => ({
    format, shortCircuit: true,
    source: esbuild.transformSync(code.replace(/import\.meta\.env/g, '__kern_meta_env'), { loader: 'jsx', jsx: 'automatic', format: format === 'module' ? 'esm' : 'cjs',
      platform: 'node', target: 'node22', sourcefile: file, sourcemap: 'inline' }).code,
  });
  if (typeof Module.registerHooks === 'function') Module.registerHooks({
    load(url, context, nextLoad) {
      if (!/\.[cm]?js$/.test(url) || !url.startsWith('file:///workspace/') || url.includes('/node_modules/')) return nextLoad(url, context);
      const file = fileURLToPath(url);
      let out;
      try { out = nextLoad(url, context); }
      catch (e) { if (!OFF.test(String(e && e.message))) throw e; return compile(file, fs.readFileSync(file, 'utf8'), context.format || 'commonjs'); }
      const given = out.source != null ? (typeof out.source === 'string' ? out.source : Buffer.from(out.source).toString('utf8')) : null;
      const code = given ?? fs.readFileSync(file, 'utf8');
      if (hasJsx(code)) return compile(file, fs.readFileSync(file, 'utf8'), out.format);
      const raw = readRaw(file, 'utf8');
      if (raw.includes('sourceMappingURL') && (given === null || given === raw)) return { ...out, source: blankMaps(raw) };
      return out;
    },
  });
  const load = Module._extensions['.js'];
  Module._extensions['.js'] = function (module, filename) {
    if (filename.startsWith('/workspace/') && !filename.includes('/node_modules/')) {
      const code = fs.readFileSync(filename, 'utf8');
      if (hasJsx(code)) return module._compile(compile(filename, code, 'commonjs').source, filename);
    }
    return load.call(this, module, filename);
  };
}

{
  const Module = createRequire(import.meta.url)('node:module');
  const NOTE = /0\s*&&\s*\(\s*module\.exports\s*=\s*\{([^{}]*)\}\s*\)/g;
  const text = (s) => (typeof s === 'string' ? s : Buffer.from(s).toString('utf8'));
  const entries = (code) => [...code.matchAll(NOTE)].flatMap((m) => m[1].split(',').map((e) => e.trim()).filter(Boolean));
  const own = (url) => url.startsWith('file:///workspace/') && !url.includes('/node_modules/');
  if (typeof Module.registerHooks === 'function') Module.registerHooks({
    load(url, context, nextLoad) {
      const out = nextLoad(url, context);
      if (out.format !== 'commonjs' || out.source == null || !own(url)) return out;
      const code = text(out.source);
      const listed = entries(code);
      const handed = new Set();
      const seen = new Set([url]);
      const follow = (file, list) => {
        for (const e of list) {
          const star = /^\.\.\.require\((["'])([^"']+)\1\)$/.exec(e);
          let target = null;
          if (star && /^\.{1,2}\//.test(star[2])) target = relativeFile(star[2], file);
          else if (star) { try { target = createRequire(file).resolve(star[2]); } catch {} }
          const href = target && pathToFileURL(target).href;
          if (!href || seen.has(href) || !own(href)) continue;
          seen.add(href);
          let inner = [];
          try { const t = nextLoad(href, context); if (t.source != null) inner = entries(text(t.source)); } catch {}
          for (const n of inner) if (/^[A-Za-z_$][\w$]*$/.test(n) && n !== 'default') handed.add(n);
          follow(target, inner);
        }
      };
      follow(fileURLToPath(url), listed);
      const extra = [...handed].filter((n) => !listed.includes(n));
      const changed = cjs.has(fileURLToPath(url));
      if (!extra.length && !changed) return out;
      let add = extra.length ? '\n;0 && (module.exports = { ' + extra.map((n) => JSON.stringify(n) + ': __kern_star').join(', ') + ' });' : '';
      if (changed) add += '\n;globalThis.__kernDriveCjs && globalThis.__kernDriveCjs(module);';
      const at = code.lastIndexOf('\n//# sourceMappingURL=');
      return { ...out, source: at < 0 ? code + add : code.slice(0, at) + add + code.slice(at) };
    },
  });
}

globalThis.__kern_meta_env = Object.assign({ MODE: 'production', DEV: false, PROD: true, SSR: true, BASE_URL: '/' },
  Object.fromEntries(Object.entries(process.env).filter(([k]) => k.startsWith('VITE_'))));

let seed = 0x2545f491 >>> 0;
const rand = () => { seed = (seed + 0x6d2b79f5) >>> 0; let t = seed; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
Math.random = rand;
const RealDate = Date;
const NOW = RealDate.UTC(2024, 0, 1, 12, 0, 0);
function FixedDate(...a) { if (!new.target) return new RealDate(NOW).toString(); return Reflect.construct(RealDate, a.length ? a : [NOW], new.target); }
FixedDate.prototype = RealDate.prototype; FixedDate.now = () => NOW; FixedDate.UTC = RealDate.UTC; FixedDate.parse = RealDate.parse;
globalThis.Date = FixedDate;
{
  const crypto = (await import('node:crypto')).default;
  const bytes = (n) => { const b = Buffer.alloc(n); for (let i = 0; i < n; i++) b[i] = Math.floor(rand() * 256); return b; };
  const fill = (a) => { new Uint8Array(a.buffer, a.byteOffset, a.byteLength).set(bytes(a.byteLength)); return a; };
  const uuid = () => { const b = bytes(16); b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80; const h = b.toString('hex'); return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`; };
  const put = (o, k, v) => { try { Object.defineProperty(o, k, { value: v, writable: true, configurable: true, enumerable: true }); } catch { try { o[k] = v; } catch {} } };
  put(crypto, 'randomBytes', (n, cb) => { const b = bytes(n); if (typeof cb === 'function') { process.nextTick(cb, null, b); return undefined; } return b; });
  put(crypto, 'randomUUID', uuid); put(crypto, 'getRandomValues', fill);
  if (globalThis.crypto) { put(globalThis.crypto, 'getRandomValues', fill); put(globalThis.crypto, 'randomUUID', uuid); }
  try { (await import('node:module')).syncBuiltinESMExports(); } catch {}
}
{
  const net = (await import('node:net')).default;
  let port = 41000;
  const listen = net.Server.prototype.listen;
  net.Server.prototype.listen = function (...a) {
    const any = (p) => p === 0 || p === '0';
    if (a.length === 0 || a[0] === undefined || a[0] === null || typeof a[0] === 'function') a.unshift(port++);
    else if (any(a[0])) a[0] = port++;
    else if (a[0] && typeof a[0] === 'object' && !(a[0] instanceof net.Server) && 'port' in a[0] && any(a[0].port)) a[0] = { ...a[0], port: port++ };
    return listen.apply(this, a);
  };
}

function digest() { return createRequire(import.meta.url)('node:crypto').createHash('sha256'); }
function text(e) {
  let n = typeof e, msg = '';
  try { if (e && (typeof e === 'object' || typeof e === 'function')) { n = String(e.name || (e.constructor && e.constructor.name) || 'Error'); msg = String(e.message !== undefined ? e.message : e); } else msg = String(e); } catch { msg = '?'; }
  const s = (n + ': ' + msg).split('/workspace/').join('').replace(/\r?\n/g, '\\n');
  return s.length > 300 ? s.slice(0, 280) + '...#' + digest().update(s).digest('hex').slice(0, 16) : s;
}
function write(v, full, sink) {
  const state = { text: '', cut: false, size: 0 };
  const STOP = Symbol('stop');
  const out = (s) => { if (sink) { state.size += s.length; if (state.size > 16 * 1024 * 1024) throw STOP; sink(s); return; } if (state.text.length < 4096) state.text += s; else state.cut = true; };
  const most = (n) => (full ? Infinity : n);
  const seen = new Set();
  const go = (x, depth) => {
    if (x === undefined) return out('undefined');
    if (x === null) return out('null');
    switch (typeof x) {
      case 'number': return out(Object.is(x, -0) ? '-0' : String(x));
      case 'bigint': return out(x + 'n');
      case 'boolean': return out(String(x));
      case 'string': { let s = x.split('/workspace/').join(''); if (s.length > most(1000)) { s = s.slice(0, 1000); state.cut = true; } return out(JSON.stringify(s)); }
      case 'symbol': return out('Symbol(' + JSON.stringify(String(x.description || '')) + ')');
      case 'function': return out('[Function ' + (x.name || 'anonymous') + ']');
    }
    if (seen.has(x)) return out('[Circular]');
    if (depth > (full ? 32 : 6)) { state.cut = true; return out('[...]'); }
    seen.add(x);
    try {
      if (x instanceof RealDate) return out('Date(' + (isNaN(x.getTime()) ? 'Invalid' : x.toISOString()) + ')');
      if (x instanceof RegExp) return out(String(x));
      if (x instanceof Error) return out('Error(' + text(x) + ')');
      if (Buffer.isBuffer(x)) { const n = Math.min(x.length, most(64)); if (n < x.length) state.cut = true; return out('Buffer(' + x.length + ':' + x.subarray(0, n).toString('hex') + ')'); }
      if (ArrayBuffer.isView(x) && typeof x.length === 'number') { const n = Math.min(x.length, most(64)); if (n < x.length) state.cut = true; return out((x.constructor && x.constructor.name) + '(' + Array.from(x.slice(0, n)).join(',') + ')'); }
      if (x instanceof Map || x instanceof Set) {
        const items = [...x]; const n = Math.min(items.length, most(100)); if (n < items.length) state.cut = true;
        out(x instanceof Map ? 'Map{' : 'Set{');
        for (let i = 0; i < n; i++) { if (i) out(','); if (x instanceof Map) { go(items[i][0], depth + 1); out('=>'); go(items[i][1], depth + 1); } else go(items[i], depth + 1); }
        return out('}');
      }
      let then = null; try { then = x.then; } catch {}
      if (typeof then === 'function') return out('Promise');
      let tag = null; try { tag = x.$$typeof; } catch {}
      if (typeof tag === 'symbol' && String(tag.description || '').startsWith('react.')) {
        let type = 'element';
        try { const t = x.type; type = typeof t === 'string' ? t : typeof t === 'symbol' ? String(t.description) : (t && (t.displayName || t.name)) || 'element'; } catch {}
        out('<' + type + ' ');
        let props = null; try { props = x.props; } catch {}
        go(props, depth + 1);
        return out('>');
      }
      if (Array.isArray(x)) { const n = Math.min(x.length, most(200)); out('['); for (let i = 0; i < n; i++) { if (i) out(','); go(x[i], depth + 1); } if (n < x.length) { state.cut = true; out(',...' + x.length); } return out(']'); }
      const proto = Object.getPrototypeOf(x);
      const label = proto === null ? 'null' : proto === Object.prototype ? '' : (proto.constructor && proto.constructor.name) || '?';
      const keys = Object.keys(x).sort(); const n = Math.min(keys.length, most(200)); if (n < keys.length) state.cut = true;
      out(label + '{');
      for (let i = 0; i < n; i++) { if (i) out(','); out(JSON.stringify(keys[i]) + ':'); let value; try { value = x[keys[i]]; } catch { out('[Throws]'); continue; } go(value, depth + 1); }
      return out('}');
    } finally { seen.delete(x); }
  };
  try { go(v, 0); } catch (e) { if (e === STOP) { if (sink) sink('[too large]'); } else { state.text = '[Unreadable]'; state.cut = false; } }
  return state;
}
function ser(v) {
  const shown = write(v, false);
  if (!shown.cut && shown.text.length <= 240) return shown.text;
  const h = digest();
  write(v, true, (s) => h.update(s));
  return shown.text.slice(0, 120) + '...#' + h.digest('hex').slice(0, 16);
}

const calls = [];
const counts = new Map();
globalThis.__kernDriveRecord = function (key, fn, self, args, target) {
  if (typeof fn !== 'function') return target ? Reflect.construct(fn, args, target) : Reflect.apply(fn, self, args);
  const n = (counts.get(key) || 0) + 1;
  counts.set(key, n);
  const entry = n <= LIMIT ? { k: key, n, a: ser(args) } : null;
  let result;
  try { result = target ? Reflect.construct(fn, args, target) : Reflect.apply(fn, self, args); }
  catch (e) { if (entry) { entry.o = 'e ' + text(e); calls.push(entry); } throw e; }
  if (entry) {
    let then = null;
    try { then = result !== null && (typeof result === 'object' || typeof result === 'function') ? result.then : null; } catch {}
    if (typeof then === 'function') {
      entry.o = 'promise';
      try { Reflect.apply(then, result, [(v) => { entry.r = 'r ' + ser(v); }, (e) => { entry.r = 'e ' + text(e); }]); } catch {}
    } else entry.o = 'r ' + ser(result);
    calls.push(entry);
  }
  return result;
};
process.on('exit', () => { try { fs.writeFileSync(process.env.KERN_DRIVE_CALLS || '/tmp/.kern-calls', calls.map((c) => JSON.stringify(c)).join('\n')); } catch {} });

if (cjs.size) {
  const Module = createRequire(import.meta.url)('node:module');
  const original = Module._load;
  const done = new WeakSet();
  const isClass = (f) => { try { return /^class\b/.test(Function.prototype.toString.call(f)); } catch { return false; } };
  const wrap = (key, f) => Object.assign(function (...a) { return globalThis.__kernDriveRecord(key, f, this, a, new.target); }, f);
  const record = (m, module) => {
    const exp = module.exports;
    if (!exp || (typeof exp !== 'object' && typeof exp !== 'function') || done.has(exp)) return exp;
    done.add(exp);
    if (typeof exp === 'function' && !isClass(exp)) {
      const w = wrap(m.path + '#default', exp);
      done.add(w);
      module.exports = w;
      return w;
    }
    for (const k of Object.keys(exp)) {
      const v = exp[k];
      if (typeof v === 'function' && !isClass(v)) { try { exp[k] = wrap(m.path + '#' + k, v); } catch {} }
    }
    return exp;
  };
  Module._load = function (request, parent, isMain) {
    const exp = original.apply(this, arguments);
    try {
      const file = Module._resolveFilename(request, parent, isMain);
      const m = cjs.get(file);
      if (m) return record(m, Module._cache[file] && Module._cache[file].exports === exp ? Module._cache[file] : { exports: exp });
    } catch {}
    return exp;
  };
  globalThis.__kernDriveCjs = (module) => { const m = cjs.get(module && module.filename); if (m) record(m, module); };
}
