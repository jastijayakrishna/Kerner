// What Kern's drive loader (main thread) and its hooks (loader thread) share.
//
// Path aliases of the package a file belongs to. tsx applies one tsconfig to every file (the root's, through Kern's override); in a
// monorepo each package often has its own (apps/web: "@/*" -> "src/*"). When an import does not resolve, the nearest tsconfig.json above
// the importing file, read with tsx's own parser (get-tsconfig, from Kern's pinned tools), names where the alias points. Only the
// project's own files are helped, never code inside node_modules.
//
// A package of the project itself that was never built: a monorepo's sibling package names files only a build step makes (main:
// dist/index.js), and the checkout has its source alone. Its source then loads instead, by the convention TypeScript monorepos follow: the
// package's `source` field, or src/<subpath> (src/index for the package itself). Only packages the package manager linked from the
// project's own folders qualify, never one from the registry.
//
// Assets, as bundlers hand them to code: a stylesheet is nothing, a CSS module maps each class to its own name, an SVG is a component
// (and its ReactComponent export), any other file is its address under /assets/. The same on the old and the new code, so they never
// show up as a difference, while code that uses them still runs.
import { getTsconfig, parseTsconfig, createPathsMatcher, createFilesMatcher } from '/kern-tools/node_modules/get-tsconfig/dist/index.mjs';
import fs from 'node:fs';
import path from 'node:path';

const EXTENSIONS = ['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs', '.json'];
const matchers = new Map();
// Each target as written, with each extension, as a folder; first, as TypeScript reads it, a .js name stands for its TypeScript source.
const SOURCE = { '.js': ['.ts', '.tsx'], '.jsx': ['.tsx'], '.mjs': ['.mts'], '.cjs': ['.cts'] };
const files = (targets) => targets.flatMap((t) => {
  const js = /\.[cm]?jsx?$/.exec(t);
  const ts = js ? SOURCE[js[0]].map((e) => t.slice(0, -js[0].length) + e) : [];
  return [...ts, t, ...EXTENSIONS.map((e) => t + e), ...EXTENSIONS.map((e) => path.join(t, 'index' + e))];
});
const helped = (specifier, importer) =>
  !!importer && importer.startsWith('/workspace/') && !importer.includes('/node_modules/') && !/^[./]/.test(specifier) && !specifier.includes(':');

/** The project's own files an import that does not resolve may mean, most likely first: a path alias of the importer's package, then
 *  the source of a package of the project that was never built. */
export function ownFiles(specifier, importer) {
  return helped(specifier, importer) ? [...aliased(specifier, importer), ...unbuilt(specifier, importer)] : [];
}

/** The file a relative import from `importer` names, as TypeScript finds it. */
export function relativeFile(specifier, importer) {
  return files([path.resolve(path.dirname(importer), specifier)]).find((f) => { try { return fs.statSync(f).isFile(); } catch { return false; } }) || null;
}

function aliased(specifier, importer) {
  let match = matchers.get(importer);
  if (match === undefined) {
    match = null;
    try { const found = tsconfigOf(importer); if (found && found.path.startsWith('/workspace/')) match = createPathsMatcher(found); } catch {}
    matchers.set(importer, match);
  }
  if (!match) return [];
  return files(match(specifier) || []).filter((p) => p.startsWith('/workspace/'));
}

// The tsconfig that applies to a file, as TypeScript's editor support picks it: the nearest one, unless it does not include the file and
// one it references does (Vite's layout: tsconfig.json has "files": [] and references tsconfig.app.json and tsconfig.node.json) and has
// aliases of its own.
const read = new Map();
function tsconfigOf(file) {
  const nearest = getTsconfig(path.dirname(file), 'tsconfig.json', read);
  if (!nearest || createFilesMatcher(nearest)(file)) return nearest;
  for (const ref of nearest.config.references || []) {
    try {
      const at = path.resolve(path.dirname(nearest.path), ref.path);
      const found = { path: at.endsWith('.json') ? at : path.join(at, 'tsconfig.json') };
      found.config = parseTsconfig(found.path, read);
      // One that has no aliases of its own leaves the nearest one's (paths kept in the tsconfig.json that lists the others).
      const own = found.config.compilerOptions || {};
      if (createFilesMatcher(found)(file) && (own.paths || own.baseUrl)) return found;
    } catch {}
  }
  return nearest;
}

function unbuilt(specifier, importer) {
  const parts = specifier.split('/');
  const n = specifier.startsWith('@') ? 2 : 1;
  const name = parts.slice(0, n).join('/');
  const sub = parts.slice(n).join('/');
  // The package as the package manager linked it into a node_modules folder above the importer: a link to the project's own folders.
  let root = null;
  for (let dir = path.dirname(importer); !root && dir.startsWith('/workspace'); dir = path.dirname(dir)) {
    try { root = fs.realpathSync(path.join(dir, 'node_modules', name)); } catch {}
  }
  if (!root || !root.startsWith('/workspace/') || root.includes('/node_modules/')) return [];
  let pkg = {};
  try { pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')); } catch {}
  const targets = !sub && typeof pkg.source === 'string' ? [path.join(root, pkg.source)] : [];
  targets.push(path.join(root, 'src', sub || 'index'));
  return files(targets).filter((p) => p.startsWith(root + '/'));
}

// Source maps are the tools' to make. A project file's own sourceMappingURL comment would tell the coverage which line each piece of its
// code is on (Node uses it, and esbuild folds it into the map it makes), so a file could put code that never ran on any line it likes.
// Each such comment, as Node finds one, is blanked: every character but line breaks becomes a space, so no position moves; inside a
// string (the match holds a quote) only the part before the quote, so the string still closes.
const MAP_COMMENT = /\/\*[#@]\s+sourceMappingURL=[^]*?\*\/|\/\/[#@]\s+sourceMappingURL=[^\s]*/g;
export const blankMaps = (text) => (!text.includes('sourceMappingURL') ? text : text.replace(MAP_COMMENT, (m) => {
  const cut = m.search(/['"`]/);
  return cut < 0 ? m.replace(/[^\r\n]/g, ' ') : m.slice(0, cut).replace(/[^\r\n]/g, ' ') + m.slice(cut);
}));

export const ASSET = /\.(css|scss|sass|less|styl|pcss|svg|png|jpe?g|gif|webp|avif|ico|bmp|woff2?|ttf|otf|eot|mp3|mp4|webm|wav|ogg|pdf|mdx?|txt|html|glsl|frag|vert|wasm|sql)([?#].*)?$/i;
// Next.js's marker packages: its bundler makes `import 'server-only'` (and 'client-only') an empty module where it is allowed, and plain
// Node has no such step, so the real package throws or is not installed. Here they are always empty.
export const MARKER = /^(server|client)-only$/;
const STYLE = /\.(css|scss|sass|less|styl|pcss)$/i;

function kind(name) {
  const file = name.replace(/[?#].*$/, '');
  if (STYLE.test(file)) return /\.module\.[a-z]+$/i.test(file) ? 'classes' : 'style';
  if (/\.svg$/i.test(file) && !/[?&]url\b/.test(name)) return 'svg';
  return 'file';
}
const address = (name) => '/assets/' + path.basename(name.replace(/[?#].*$/, ''));
// A CSS module's class names: any name maps to itself; nothing pretends to be a promise or a module record.
const classes = () => new Proxy({}, { get: (_, k) => (typeof k === 'string' && k !== 'then' && k !== '__esModule' && k !== 'default' ? k : undefined) });

/** An asset as an ES module's source. */
export function assetModule(name) {
  switch (kind(name)) {
    case 'style': return 'export default {};';
    case 'classes': return "export default new Proxy({}, { get: (_, k) => (typeof k === 'string' && k !== 'then' ? k : undefined) });";
    case 'svg': return 'const Svg = function SvgAsset() { return null; }; export default Svg; export const ReactComponent = Svg;';
    default: return 'export default ' + JSON.stringify(address(name)) + ';';
  }
}

/** An asset as a CommonJS module's exports, shaped as compiled ES imports read them (`__esModule`, `default`). */
export function assetExports(name) {
  switch (kind(name)) {
    case 'style': return {};
    case 'classes': return { __esModule: true, default: classes() };
    case 'svg': { const Svg = function SvgAsset() { return null; }; return { __esModule: true, default: Svg, ReactComponent: Svg }; }
    default: return { __esModule: true, default: address(name) };
  }
}
