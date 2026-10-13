import { spawnSync } from 'node:child_process';
import { closeSync, copyFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync, openSync, readdirSync, realpathSync, rmSync, statSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';

// Files whose change means the old version needs packages of its own.
const PACKAGE_FILES = new Set(['package.json', 'package-lock.json', 'npm-shrinkwrap.json', 'pnpm-lock.yaml', 'pnpm-workspace.yaml', 'yarn.lock', '.yarnrc.yml', '.npmrc', 'bun.lock', 'bun.lockb']);
const WINDOWS = process.platform === 'win32';

function run(command, args, cwd, { shell = false, stdin = 'pipe' } = {}) {
  const result = spawnSync(command, args, { cwd, encoding: 'utf8', maxBuffer: 64 << 20, shell, windowsHide: true, stdio: [stdin, 'pipe', 'pipe'] });
  if (result.error) throw new Error(`${command} could not start: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`${command} ${args.join(' ')} failed (exit ${result.status}): ${(result.stderr || result.stdout).trim().slice(0, 500)}`);
  return result.stdout.trim();
}

/** The package manager the old version's files ask for, and how it installs exactly what its lockfile says. */
function installer(dir) {
  if (existsSync(join(dir, 'pnpm-lock.yaml'))) return ['pnpm', ['install', '--frozen-lockfile']];
  if (existsSync(join(dir, 'yarn.lock'))) return ['yarn', existsSync(join(dir, '.yarnrc.yml')) ? ['install', '--immutable'] : ['install', '--frozen-lockfile']];
  if (existsSync(join(dir, 'bun.lock')) || existsSync(join(dir, 'bun.lockb'))) return ['bun', ['install', '--frozen-lockfile']];
  if (existsSync(join(dir, 'package-lock.json')) || existsSync(join(dir, 'npm-shrinkwrap.json'))) return ['npm', ['ci', '--no-audit', '--no-fund']];
  return ['npm', ['install', '--no-audit', '--no-fund']];
}

const DIR_LINK = WINDOWS ? 'junction' : 'dir';

/** Whether a real path is the checkout's own code: inside `root` and outside every node_modules folder (a workspace package, say). */
function ownCode(root, path) {
  const rel = relative(root, path);
  return rel !== '' && !rel.startsWith('..') && !isAbsolute(rel) && !rel.split(sep).includes('node_modules');
}

/** The entries of a node_modules folder, a scoped package (`@scope/name`) as its own entry: [name, path]. */
function entriesOf(folder) {
  const out = [];
  for (const entry of readdirSync(folder, { withFileTypes: true })) {
    const path = join(folder, entry.name);
    if (entry.name.startsWith('@') && entry.isDirectory()) for (const scoped of readdirSync(path)) out.push([`${entry.name}/${scoped}`, join(path, scoped)]);
    else out.push([entry.name, path]);
  }
  return out;
}

/** A node_modules folder's links to the checkout's own code, as Map(name → real path): the workspace packages npm, yarn and pnpm link in. */
function workspaceLinks(root, folder) {
  const out = new Map();
  for (const [name, path] of entriesOf(folder)) {
    if (!lstatSync(path).isSymbolicLink()) continue;
    let real;
    try {
      real = realpathSync(path);
    } catch {
      continue;
    }
    if (ownCode(root, real)) out.set(name, real);
  }
  return out;
}

/**
 * Gives the old version (in `target`) the new checkout's node_modules folder `folder`. A folder with no workspace links is linked whole.
 * A folder with workspace links becomes a real folder: each installed package is linked to the new checkout's copy, but each workspace
 * package to the old version's own copy, which git archive wrote. Linked whole, its workspace links would point at the new checkout's
 * code, and the old version would run the new copy of a workspace package: a change there could never show as a difference.
 */
function linkModules(root, real, target, folder) {
  const source = join(root, folder);
  const link = join(target, folder);
  const own = workspaceLinks(real, source);
  if (!own.size) {
    symlinkSync(source, link, DIR_LINK);
    return;
  }
  mkdirSync(link);
  for (const [name, path] of entriesOf(source)) {
    const to = join(link, name);
    mkdirSync(dirname(to), { recursive: true });
    if (own.has(name)) symlinkSync(join(target, relative(real, own.get(name))), to, DIR_LINK);
    // A file in node_modules (.package-lock.json, .modules.yaml) is copied: a link to a file needs rights Windows rarely gives.
    else if (statSync(path).isFile()) copyFileSync(path, to);
    else symlinkSync(path, to, DIR_LINK);
  }
}

/** Every node_modules folder of a checkout, outside other node_modules folders and git's store, at most four folders deep. */
function moduleFolders(root, dir = root, depth = 0, out = []) {
  if (depth > 4) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.name === '.git') continue;
    const path = join(dir, entry.name);
    if (entry.name === 'node_modules') out.push(relative(root, path));
    else moduleFolders(root, path, depth + 1, out);
  }
  return out;
}

/**
 * Writes the old version (commit `base` of `repo`) into the new folder `out`, beside the new one, with the packages it needs:
 * - the new version's own, linked, when no package file changed since `base` (or with install 'never');
 * - a clean install from the old version's lockfile otherwise (or with install 'always').
 *
 * `build` is a command run in `out` afterwards. Answers what was done.
 */
export function buildOld({ repo, base, out, install = 'auto', build = null }) {
  const root = resolve(repo);
  const target = resolve(out);
  if (existsSync(target)) throw new Error(`${target} already exists: kern-twin build writes a new folder`);
  const sha = run('git', ['-C', root, 'rev-parse', '--verify', `${base}^{commit}`], root);
  const scratch = mkdtempSync(join(tmpdir(), 'kern-twin-build-'));
  try {
    const archive = join(scratch, 'old.tar');
    run('git', ['-C', root, 'archive', '--format=tar', '-o', archive, sha], root);
    mkdirSync(target, { recursive: true });
    // tar reads the archive on its standard input and unpacks into its working folder, so no path reaches it: the GNU tar that Git Bash
    // puts first on Windows reads C:\...\old.tar as a remote host "C", and a folder after -C fails the same way.
    const input = openSync(archive, 'r');
    try {
      run('tar', ['-xf', '-'], target, { stdin: input });
    } finally {
      closeSync(input);
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
  // What changed between the old commit and the new version as it is on disk.
  const changed = run('git', ['-C', root, 'diff', '--name-only', sha], root).split('\n').filter((path) => path && PACKAGE_FILES.has(basename(path)));
  const fresh = install === 'always' || (install === 'auto' && changed.length > 0);
  let dependencies;
  let manager = null;
  if (fresh) {
    const [command, args] = installer(target);
    run(command, args, target, { shell: WINDOWS });
    dependencies = 'installed';
    manager = `${command} ${args.join(' ')}`;
  } else {
    const real = realpathSync(root);
    for (const folder of moduleFolders(root)) {
      const link = join(target, folder);
      if (!existsSync(dirname(link)) || existsSync(link)) continue;
      linkModules(root, real, target, folder);
    }
    dependencies = 'linked';
  }
  if (build) run(build, [], target, { shell: true });
  return { status: 'BUILT', base: sha, out: target, dependencies, manager, changed };
}
