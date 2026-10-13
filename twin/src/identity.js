import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { readdir, readFile, realpath, stat } from 'node:fs/promises';
import { dirname, join, relative, sep } from 'node:path';

// What is not the version's own code: git's store and Kern's own working files. Installed packages (node_modules) are not walked
// either, since the lockfile names them; but a package linked into node_modules from outside every node_modules folder is the team's
// own code (a workspace package of a pnpm, yarn or npm monorepo, or a file: dependency), and it is walked.
const SKIP = new Set(['.git', '.kern-twin']);
// The files that make up what a version does: its code (JavaScript and TypeScript, components, native add-ons, WebAssembly), the
// templates it renders, and the data and schemas it reads (JSON, YAML, TOML, SQL, GraphQL, Prisma, protobuf). A change to any of them
// is a new version. It is an explicit list, so the files an app writes while it runs (logs, uploads) never change the name.
const CODE = /\.(?:[cm]?[jt]sx?|json|vue|svelte|astro|node|wasm|ejs|hbs|handlebars|pug|jade|mustache|njk|liquid|eta|twig|html?|ya?ml|toml|sql|graphqls?|gql|prisma|proto)$/i;
const MOST_FILES = 20000;

const byName = (a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
/** Whether a real path lies inside a node_modules folder: an installed package, named by the lockfile. */
const installed = (path) => path.split(sep).some((part) => part.toLowerCase() === 'node_modules');

/** The folder of the nearest package.json above `file`: where the version's own code lives. */
export function packageRoot(file) {
  let dir = dirname(file);
  for (;;) {
    if (existsSync(join(dir, 'package.json'))) return dir;
    const up = dirname(dir);
    if (up === dir) return dirname(file);
    dir = up;
  }
}

/**
 * The links in one node_modules folder, scoped packages (`@scope/name`) included, as [name, path]. A folder that cannot be read holds
 * nothing the app could load from it either.
 */
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

/**
 * A name for one version of the code: sha256 over its entry point and over the path and content of every code, template and data file
 * of its package (see CODE; lockfiles included), and of every workspace package linked into a node_modules folder it can load from:
 * its own, those inside it and inside linked packages, and those of every folder above it, as Node looks. Any change to them is a new
 * candidate, so evidence never carries over to code it was not gathered on.
 */
export async function versionId(file, exportName) {
  const root = packageRoot(file);
  // Each file as [its name in the version, its path]. A linked package's files are named by where the link sits, so the name does not
  // depend on where the monorepo is checked out.
  const files = [];
  // The real paths of the folders walked: a link that loops back to one of them ends there.
  const walked = new Set();
  // The node_modules folders to look in for linked packages, with the name each has in the version.
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
  // A link inside the version (src/shared -> ../../shared) leads to code the app loads by a relative path: it is followed, unless it
  // leads into an installed package. A link back to a folder already walked ends there.
  const follow = async (path, shown) => {
    const real = await realpath(path).catch(() => null);
    if (!real || installed(real)) return;
    const info = await stat(real).catch(() => null);
    if (info?.isDirectory()) await walk(real, shown);
    else if (info?.isFile() && CODE.test(real)) files.push([shown, real]);
  };
  const top = await realpath(root);
  await walk(top, '');
  // Node looks for a package in the node_modules of every folder above too: a monorepo's own node_modules holds its workspace links.
  let up = '';
  for (let dir = top; dirname(dir) !== dir;) {
    dir = dirname(dir);
    up += '../';
    modules.push([join(dir, 'node_modules'), `${up}node_modules`]);
  }
  // Walking a linked package may find more node_modules folders, with links of their own; they join the end of the list.
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
