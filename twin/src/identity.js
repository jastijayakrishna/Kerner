import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { readdir, readFile, realpath, stat } from 'node:fs/promises';
import { dirname, join, relative, sep } from 'node:path';

const SKIP = new Set(['.git', '.kern-twin']);
const CODE = /\.(?:[cm]?[jt]sx?|json|vue|svelte|astro|node|wasm|ejs|hbs|handlebars|pug|jade|mustache|njk|liquid|eta|twig|html?|ya?ml|toml|sql|graphqls?|gql|prisma|proto)$/i;
const MOST_FILES = 20000;

const byName = (a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
const installed = (path) => path.split(sep).some((part) => part.toLowerCase() === 'node_modules');

export function packageRoot(file) {
  let dir = dirname(file);
  for (;;) {
    if (existsSync(join(dir, 'package.json'))) return dir;
    const up = dirname(dir);
    if (up === dir) return dirname(file);
    dir = up;
  }
}

async function linksIn(folder) {
  const list = async (dir) => (await readdir(dir, { withFileTypes: true }).catch(() => [])).sort(byName);
  const out = [];
  for (const entry of await list(folder)) {
    const path = join(folder, entry.name);
    if (entry.isSymbolicLink()) out.push([entry.name, path]);
    else if (entry.name.startsWith('@') && entry.isDirectory()) {
      for (const scoped of await list(path)) if (scoped.isSymbolicLink()) out.push([`${entry.name}/${scoped.name}`, join(path, scoped.name)]);
    }
  }
  return out;
}

export async function versionId(file, exportName) {
  const root = packageRoot(file);
  const files = [];
  const walked = new Set();
  const modules = [];
  const walk = async (dir, name) => {
    if (walked.has(dir)) return;
    walked.add(dir);
    for (const entry of (await readdir(dir, { withFileTypes: true })).sort(byName)) {
      const path = join(dir, entry.name);
      const shown = name ? `${name}/${entry.name}` : entry.name;
      if (entry.name === 'node_modules') modules.push([path, shown]);
      else if (SKIP.has(entry.name)) continue;
      else if (entry.isDirectory()) await walk(path, shown);
      else if (entry.isFile() && CODE.test(entry.name)) files.push([shown, path]);
      else if (entry.isSymbolicLink()) await follow(path, shown);
      if (files.length > MOST_FILES) throw new Error(`more than ${MOST_FILES} code files under ${root} and the packages it links: give the candidate an id`);
    }
  };
  const follow = async (path, shown) => {
    const real = await realpath(path).catch(() => null);
    if (!real || installed(real)) return;
    const info = await stat(real).catch(() => null);
    if (info?.isDirectory()) await walk(real, shown);
    else if (info?.isFile() && CODE.test(real)) files.push([shown, real]);
  };
  const top = await realpath(root);
  await walk(top, '');
  let up = '';
  for (let dir = top; dirname(dir) !== dir;) {
    dir = dirname(dir);
    up += '../';
    modules.push([join(dir, 'node_modules'), `${up}node_modules`]);
  }
  for (let i = 0; i < modules.length; i++) {
    const [folder, shown] = modules[i];
    for (const [name, path] of await linksIn(folder)) {
      const real = await realpath(path).catch(() => null);
      if (!real || installed(real) || !(await stat(real).catch(() => null))?.isDirectory()) continue;
      await walk(real, `${shown}/${name}`);
    }
  }
  const hash = createHash('sha256').update(`entry ${relative(root, file).split(sep).join('/')}#${exportName}\0`);
  for (const [name, path] of files.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    hash.update(`${name}\0${createHash('sha256').update(await readFile(path)).digest('hex')}\0`);
  }
  return `sha256:${hash.digest('hex')}`;
}
