// Kern's drive hooks, registered by drive-loader.mjs; they run in Node's loader thread. Every import of a changed ES module resolves to
// a small wrapper (at a kern-wrap: address) whose exports are hoisted functions that record each call and hand it to the real module; only the wrapper imports the real module, which keeps its own address, so line coverage, source maps,
// relative imports and import.meta.url are all the real file's. The driver's package imports resolve as if it sat at the project's root.
// The driver's stand-ins (/kern/stubs/<package>.mjs) replace packages that cannot load here, for every importer but the stand-in itself,
// when the name does not resolve or resolves into node_modules: never Node's own modules or the project's code. An import that does not
// resolve may be a path alias of the importer's own package or a package of the project that was never built, and assets load as
// bundlers hand them over (drive-support.mjs). Vite's import.meta.env becomes a plain object.
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
  // `export *` never hands on a default export: one that is no function Kern records (memo(Page), a reducer, a class) is handed on as it
  // is, exactly what an importer of the module itself gets, so a side where the module has no wrapper sees the same.
  if (m.hasDefault && !m.functions.includes('default')) s += 'export { default } from ' + JSON.stringify(m.url) + ';\n';
  // A module tsx loads as CommonJS (its package has no "type": "module") reaches the wrapper with its whole exports object as the default;
  // the function to call is that object's `default` when it says __esModule, the one an importer of the module itself is handed.
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
    // The project's own code is never stood in for: a path alias of the importer's own package (`@lib/auth` in apps/web's tsconfig), a
    // package of the project.
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
  // Everyone but the wrapper itself gets the wrapper.
  if (m && !byWrap.has(parent)) return { url: m.wrap, shortCircuit: true };
  return r;
}

export async function load(url, context, next) {
  const m = byWrap.get(url);
  if (m) return { format: 'module', source: wrapper(m), shortCircuit: true };
  const out = await next(url, context);
  if (url.startsWith('file:') && !url.includes('/node_modules/') && out.source != null) {
    const s = typeof out.source === 'string' ? out.source : new TextDecoder().decode(out.source);
    // The same length (15 characters), so every position V8 reports for line coverage still points at the file's own text; a project
    // file's own source map is blanked the same way (drive-support.mjs).
    let t = s.includes('import.meta.env') ? s.replace(/import\.meta\.env/g, '__kern_meta_env') : s;
    if (url.startsWith('file:///workspace/')) t = blankMaps(t);
    if (t !== s) return { ...out, source: t };
  }
  return out;
}
