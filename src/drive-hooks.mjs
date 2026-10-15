import { fileURLToPath, pathToFileURL } from 'node:url';
import { ownFiles, blankMaps, ASSET, MARKER, assetModule } from './drive-support.mjs';

const DRIVER = 'file:///kern/driver.mjs';
const STUBS = 'file:///kern/stubs/';
const ROOT = 'file:///workspace/kern-driver.mjs';
const byUrl = new Map();
const byWrap = new Map();
const stubs = new Map();

export function initialize(data) {
  for (const m of data.esm) { byUrl.set(m.url, m); byWrap.set(m.wrap, m); }
  for (const [name, url] of Object.entries(data.stubs || {})) stubs.set(name, url);
}

const identifier = (n) => /^[A-Za-z_$][\w$]*$/.test(n);
const bare = (s) => !/^(\.|\/|file:|node:|data:|kern-wrap:)/.test(s);

function wrapper(m) {
  let s = 'import * as __kern_real from ' + JSON.stringify(m.url) + ';\nexport * from ' + JSON.stringify(m.url) + ';\nconst __kern_rec = globalThis.__kernDriveRecord;\n';
  if (m.hasDefault && !m.functions.includes('default')) s += 'export { default } from ' + JSON.stringify(m.url) + ';\n';
  if (m.functions.includes('default')) s += 'const __kern_default = () => { const d = __kern_real.default; return d !== null && typeof d === "object" && d.__esModule === true && "default" in d ? d.default : d; };\n';
  for (const f of m.functions) {
    const key = JSON.stringify(m.path + '#' + f);
    if (f === 'default') s += 'export default function (...a) { return __kern_rec(' + key + ', __kern_default(), this, a, new.target); }\n';
    else if (identifier(f) && !f.startsWith('__kern')) s += 'export function ' + f + '(...a) { return __kern_rec(' + key + ', __kern_real.' + f + ', this, a, new.target); }\n';
  }
  return s;
}

export async function resolve(specifier, context, next) {
  if (ASSET.test(specifier)) return { url: 'data:text/javascript,' + encodeURIComponent(assetModule(specifier)), shortCircuit: true };
  if (MARKER.test(specifier)) return { url: 'data:text/javascript,export%20%7B%7D%3B', shortCircuit: true };
  if (byWrap.has(specifier)) return { url: specifier, shortCircuit: true };
  const parent = context.parentURL;
  const ctx = (parent === DRIVER || (parent || '').startsWith(STUBS)) && bare(specifier) ? { ...context, parentURL: ROOT } : context;
  const importer = (ctx.parentURL || '').startsWith('file:') ? fileURLToPath(ctx.parentURL) : '';
  const stub = stubs.get(specifier);
  if (stub && parent !== stub) {
    let real = null;
    try { real = await next(specifier, ctx); } catch {}
    for (const file of real ? [] : ownFiles(specifier, importer)) {
      try { real = await next(pathToFileURL(file).href, ctx); break; } catch {}
    }
    if (!real || real.url.includes('/node_modules/')) return { url: stub, shortCircuit: true };
  }
  let r;
  try { r = await next(specifier, ctx); }
  catch (e) {
    for (const file of bare(specifier) ? ownFiles(specifier, importer) : []) {
      try { r = await next(pathToFileURL(file).href, ctx); break; } catch {}
    }
    if (!r) throw e;
  }
  const m = byUrl.get(r.url);
  if (m && !byWrap.has(parent)) return { url: m.wrap, shortCircuit: true };
  return r;
}

export async function load(url, context, next) {
  const m = byWrap.get(url);
  if (m) return { format: 'module', source: wrapper(m), shortCircuit: true };
  const out = await next(url, context);
  if (url.startsWith('file:') && !url.includes('/node_modules/') && out.source != null) {
    const s = typeof out.source === 'string' ? out.source : new TextDecoder().decode(out.source);
    let t = s.includes('import.meta.env') ? s.replace(/import\.meta\.env/g, '__kern_meta_env') : s;
    if (url.startsWith('file:///workspace/')) t = blankMaps(t);
    if (t !== s) return { ...out, source: t };
  }
  return out;
}
