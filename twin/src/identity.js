import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, join, relative, sep } from 'node:path';

// What is not the version's own code: installed packages (the lockfile names them), git's store, and Kern's own working files.
const SKIP = new Set(['node_modules', '.git', '.kern-twin']);
const CODE = /\.(?:[cm]?[jt]sx?|json)$/;
const MOST_FILES = 20000;

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
 * A name for one version of the code: sha256 over its entry point and over the path and content of every code file of its package
 * (JavaScript, TypeScript and JSON, lockfiles included). Any change to the code is a new candidate, so evidence never carries over
 * to code it was not gathered on.
 */
export async function versionId(file, exportName) {
  const root = packageRoot(file);
  const files = [];
  const walk = async (dir) => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (SKIP.has(entry.name)) continue;
      const path = join(dir, entry.name);
      if (entry.isDirectory()) await walk(path);
      else if (entry.isFile() && CODE.test(entry.name)) files.push(path);
      if (files.length > MOST_FILES) throw new Error(`more than ${MOST_FILES} code files under ${root}: give the candidate an id`);
    }
  };
  await walk(root);
  const hash = createHash('sha256').update(`entry ${relative(root, file).split(sep).join('/')}#${exportName}\0`);
  for (const path of files.sort()) {
    const name = relative(root, path).split(sep).join('/');
    hash.update(`${name}\0${createHash('sha256').update(await readFile(path)).digest('hex')}\0`);
  }
  return `sha256:${hash.digest('hex')}`;
}
