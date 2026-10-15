import { getTsconfig, parseTsconfig, createPathsMatcher, createFilesMatcher } from '/kern-tools/node_modules/get-tsconfig/dist/index.mjs';
import fs from 'node:fs';
import path from 'node:path';

const EXTENSIONS = ['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs', '.json'];
const matchers = new Map();
const SOURCE = { '.js': ['.ts', '.tsx'], '.jsx': ['.tsx'], '.mjs': ['.mts'], '.cjs': ['.cts'] };
const files = (targets) => targets.flatMap((t) => {
  const js = /\.[cm]?jsx?$/.exec(t);
  const ts = js ? SOURCE[js[0]].map((e) => t.slice(0, -js[0].length) + e) : [];
  return [...ts, t, ...EXTENSIONS.map((e) => t + e), ...EXTENSIONS.map((e) => path.join(t, 'index' + e))];
});
const helped = (specifier, importer) =>
  !!importer && importer.startsWith('/workspace/') && !importer.includes('/node_modules/') && !/^[./]/.test(specifier) && !specifier.includes(':');

export function ownFiles(specifier, importer) {
  return helped(specifier, importer) ? [...aliased(specifier, importer), ...unbuilt(specifier, importer)] : [];
}

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

const read = new Map();
function tsconfigOf(file) {
  const nearest = getTsconfig(path.dirname(file), 'tsconfig.json', read);
  if (!nearest || createFilesMatcher(nearest)(file)) return nearest;
  for (const ref of nearest.config.references || []) {
    try {
      const at = path.resolve(path.dirname(nearest.path), ref.path);
      const found = { path: at.endsWith('.json') ? at : path.join(at, 'tsconfig.json') };
      found.config = parseTsconfig(found.path, read);
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

const MAP_COMMENT = /\/\*[#@]\s+sourceMappingURL=[^]*?\*\/|\/\/[#@]\s+sourceMappingURL=[^\s]*/g;
export const blankMaps = (text) => (!text.includes('sourceMappingURL') ? text : text.replace(MAP_COMMENT, (m) => {
  const cut = m.search(/['"`]/);
  return cut < 0 ? m.replace(/[^\r\n]/g, ' ') : m.slice(0, cut).replace(/[^\r\n]/g, ' ') + m.slice(cut);
}));

export const ASSET = /\.(css|scss|sass|less|styl|pcss|svg|png|jpe?g|gif|webp|avif|ico|bmp|woff2?|ttf|otf|eot|mp3|mp4|webm|wav|ogg|pdf|mdx?|txt|html|glsl|frag|vert|wasm|sql)([?#].*)?$/i;
export const MARKER = /^(server|client)-only$/;
const STYLE = /\.(css|scss|sass|less|styl|pcss)$/i;

function kind(name) {
  const file = name.replace(/[?#].*$/, '');
  if (STYLE.test(file)) return /\.module\.[a-z]+$/i.test(file) ? 'classes' : 'style';
  if (/\.svg$/i.test(file) && !/[?&]url\b/.test(name)) return 'svg';
  return 'file';
}
const address = (name) => '/assets/' + path.basename(name.replace(/[?#].*$/, ''));
const classes = () => new Proxy({}, { get: (_, k) => (typeof k === 'string' && k !== 'then' && k !== '__esModule' && k !== 'default' ? k : undefined) });

export function assetModule(name) {
  switch (kind(name)) {
    case 'style': return 'export default {};';
    case 'classes': return "export default new Proxy({}, { get: (_, k) => (typeof k === 'string' && k !== 'then' ? k : undefined) });";
    case 'svg': return 'const Svg = function SvgAsset() { return null; }; export default Svg; export const ReactComponent = Svg;';
    default: return 'export default ' + JSON.stringify(address(name)) + ';';
  }
}

export function assetExports(name) {
  switch (kind(name)) {
    case 'style': return {};
    case 'classes': return { __esModule: true, default: classes() };
    case 'svg': { const Svg = function SvgAsset() { return null; }; return { __esModule: true, default: Svg, ReactComponent: Svg }; }
    default: return { __esModule: true, default: address(name) };
  }
}
