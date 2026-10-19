'use strict';
const fs = require('fs');
const crypto = require('crypto');
const FILE = process.env.KERN_TRACE;
const CALLS_PER_KEY_MAX = 200;
const BYTES_MAX = 8 << 20;
const SHOWN_CHARS_MAX = 240;
const VALUE_CHARS_MAX = 65536;
const STOP = Symbol('stop');
const KEY = Symbol.for('kern.trace.key');
const state = { counts: new Map(), bytes: 0 };

function text(e) {
  try { return String(e && typeof e === 'object' && 'message' in e ? (e.name || 'Error') + ': ' + e.message : e).slice(0, 200); } catch { return '[Unreadable]'; }
}
function write(v, full, sink) {
  const s = { text: '', cut: false };
  const most = (n) => (full ? n * 50 : n);
  const out = (t) => { if (sink) { sink(t); return; } s.text += t; if (s.text.length > SHOWN_CHARS_MAX * 4) throw STOP; };
  const seen = new Set();
  const go = (x, depth) => {
    if (x === null) return out('null');
    const t = typeof x;
    if (t === 'undefined') return out('undefined');
    if (t === 'number') return out(Number.isNaN(x) ? 'NaN' : Object.is(x, -0) ? '-0' : String(x));
    if (t === 'bigint') return out(String(x) + 'n');
    if (t === 'boolean' || t === 'symbol') return out(String(x));
    if (t === 'string') { if (x.length > most(400)) s.cut = true; return out(JSON.stringify(x.slice(0, most(400)))); }
    if (t === 'function') return out('[function ' + (x.name || '') + ']');
    if (seen.has(x)) return out('[Circular]');
    if (depth > (full ? 32 : 6)) { s.cut = true; return out('[...]'); }
    seen.add(x);
    try {
      const tag = Object.prototype.toString.call(x);
      if (tag === '[object Date]') return out('Date(' + (isNaN(x.getTime()) ? 'Invalid' : x.toISOString()) + ')');
      if (tag === '[object RegExp]') return out(String(x));
      if (tag === '[object Error]' || x instanceof Error) return out('Error(' + text(x) + ')');
      if (tag === '[object Promise]') return out('Promise');
      if (typeof Buffer !== 'undefined' && Buffer.isBuffer(x)) { const n = Math.min(x.length, most(64)); if (n < x.length) s.cut = true; return out('Buffer(' + x.length + ':' + x.subarray(0, n).toString('hex') + ')'); }
      if (ArrayBuffer.isView(x) && typeof x.length === 'number') { const n = Math.min(x.length, most(64)); if (n < x.length) s.cut = true; return out((x.constructor && x.constructor.name) + '(' + Array.from(x.slice(0, n)).join(',') + ')'); }
      if (tag === '[object Map]' || tag === '[object Set]') {
        const items = [...x]; const n = Math.min(items.length, most(100)); if (n < items.length) s.cut = true;
        out(tag === '[object Map]' ? 'Map{' : 'Set{');
        for (let i = 0; i < n; i++) { if (i) out(','); if (tag === '[object Map]') { go(items[i][0], depth + 1); out('=>'); go(items[i][1], depth + 1); } else go(items[i], depth + 1); }
        return out('}');
      }
      let react = null; try { react = x.$$typeof; } catch {}
      if (typeof react === 'symbol' && String(react.description || '').startsWith('react.')) {
        let type = 'element';
        try { const k = x.type; type = typeof k === 'string' ? k : typeof k === 'symbol' ? String(k.description) : (k && (k.displayName || k.name)) || 'element'; } catch {}
        out('<' + type + ' ');
        let props = null; try { props = x.props; } catch {}
        go(props, depth + 1);
        return out('>');
      }
      if (Array.isArray(x)) { const n = Math.min(x.length, most(200)); out('['); for (let i = 0; i < n; i++) { if (i) out(','); go(x[i], depth + 1); } if (n < x.length) { s.cut = true; out(',...' + x.length); } return out(']'); }
      const proto = Object.getPrototypeOf(x);
      const label = proto === null ? 'null' : proto === Object.prototype ? '' : (proto.constructor && proto.constructor.name) || '?';
      const keys = Object.keys(x).sort(); const n = Math.min(keys.length, most(200)); if (n < keys.length) s.cut = true;
      out(label + '{');
      for (let i = 0; i < n; i++) { if (i) out(','); out(JSON.stringify(keys[i]) + ':'); let value; try { value = x[keys[i]]; } catch { out('[Throws]'); continue; } go(value, depth + 1); }
      return out('}');
    } finally { seen.delete(x); }
  };
  try { go(v, 0); } catch (e) { if (e === STOP) s.cut = true; else { s.text = '[Unreadable]'; s.cut = false; } }
  return s;
}
function shown(v) {
  const brief = write(v, false);
  if (!brief.cut && brief.text.length <= SHOWN_CHARS_MAX) return brief.text;
  const h = crypto.createHash('sha256');
  write(v, true, (t) => h.update(t));
  return brief.text.slice(0, 120) + '...#' + h.digest('hex').slice(0, 16);
}
function record(entry) {
  const line = JSON.stringify(entry) + '\n';
  if (state.bytes + line.length > BYTES_MAX) return;
  state.bytes += line.length;
  try { fs.appendFileSync(FILE, line); } catch {}
}
function call(key, run, args) {
  const n = (state.counts.get(key) || 0) + 1;
  state.counts.set(key, n);
  if (n > CALLS_PER_KEY_MAX) return run();
  const entry = { k: key, a: shown(args) };
  let result;
  try { result = run(); } catch (e) { entry.o = 'throws ' + text(e); record(entry); throw e; }
  if (Object.prototype.toString.call(result) === '[object Promise]') {
    try { Promise.prototype.then.call(result, (v) => { entry.o = 'resolves ' + shown(v); record(entry); }, (e) => { entry.o = 'rejects ' + text(e); record(entry); }); } catch {}
  } else { entry.o = 'returns ' + shown(result); record(entry); }
  return result;
}
function wrap(file, name, f) {
  if (typeof f !== 'function' || !FILE) return f;
  const key = file + '#' + name;
  try {
    const watched = new Proxy(f, {
      apply(target, self, args) { return call(key, () => Reflect.apply(target, self, args), args); },
      construct(target, args, newTarget) { return call(key, () => Reflect.construct(target, args, newTarget), args); },
    });
    try { Object.defineProperty(f, KEY, { value: key }); } catch {}
    return watched;
  } catch { return f; }
}
function methods(file, name, C) {
  if (typeof C !== 'function' || !FILE) return C;
  for (const [target, sep] of [[C.prototype, '#'], [C, '.']]) {
    if (!target || typeof target !== 'object' && typeof target !== 'function') continue;
    for (const m of Object.getOwnPropertyNames(target)) {
      if (['constructor', 'prototype', 'length', 'name', 'caller', 'arguments'].includes(m)) continue;
      let d; try { d = Object.getOwnPropertyDescriptor(target, m); } catch { continue; }
      if (!d || typeof d.value !== 'function' || !d.writable || !d.configurable) continue;
      try { Object.defineProperty(target, m, { ...d, value: wrap(file, name + sep + m, d.value) }); } catch {}
    }
  }
  return C;
}
function exported(file, value) {
  if (!FILE) return value;
  if (typeof value === 'function') return wrap(file, 'default', value);
  if (value && typeof value === 'object' && !Object.isFrozen(value)) {
    for (const k of Object.keys(value)) {
      let d; try { d = Object.getOwnPropertyDescriptor(value, k); } catch { continue; }
      if (d && typeof d.value === 'function' && d.writable) { try { value[k] = wrap(file, k, d.value); } catch {} }
    }
  }
  return value;
}
function json(value) {
  try { const t = JSON.stringify(value); return typeof t === 'string' && t.length <= VALUE_CHARS_MAX ? t : undefined; } catch { return undefined; }
}
function bytes(value) {
  const keys = Object.keys(value);
  return keys.length === 2 && value.type === 'Buffer' && Array.isArray(value.data) && value.data.every((b) => Number.isInteger(b) && b >= 0 && b < 256);
}
function revive(value) {
  if (Array.isArray(value)) return value.map(revive);
  if (!value || typeof value !== 'object') return value;
  if (bytes(value)) return Buffer.from(value.data);
  const out = {};
  for (const k of Object.keys(value)) Object.defineProperty(out, k, { value: revive(value[k]), enumerable: true, writable: true, configurable: true });
  return out;
}
async function probe(id, mod, name, args) {
  let o, v, k, f;
  const self = name === 'default' ? undefined : mod;
  try { f = name === 'default' ? (typeof mod === 'function' ? mod : mod.default) : mod[name]; } catch {}
  try { k = typeof f === 'function' ? f[KEY] : undefined; } catch {}
  try { record(typeof k === 'string' ? { p: String(id), s: 1, k } : { p: String(id), s: 1 }); } catch {}
  try {
    const result = Reflect.apply(f, self, revive(args));
    if (Object.prototype.toString.call(result) === '[object Promise]') {
      try { const value = await result; o = 'resolves ' + shown(value); v = json(value); } catch (e) { o = 'rejects ' + text(e); }
    } else { o = 'returns ' + shown(result); v = json(result); }
  } catch (e) { o = 'throws ' + text(e); }
  const entry = { p: String(id), o };
  if (v !== undefined) entry.v = v;
  if (typeof k === 'string') entry.k = k;
  try { record(entry); } catch {}
}
module.exports = { fn: wrap, value: wrap, class: methods, exports: exported, probe };
