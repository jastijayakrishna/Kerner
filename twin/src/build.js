import { spawnSync } from 'node:child_process';
import { closeSync, copyFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync, openSync, readdirSync, realpathSync, rmSync, statSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';

const PACKAGE_FILES = new Set(['package.json', 'package-lock.json', 'npm-shrinkwrap.json', 'pnpm-lock.yaml', 'pnpm-workspace.yaml', 'yarn.lock', '.yarnrc.yml', '.npmrc', 'bun.lock', 'bun.lockb']);
const WINDOWS = process.platform === 'win32';

function run(command, args, cwd, { shell = false, stdin = 'pipe' } = {}) {
  const result = spawnSync(command, args, { cwd, encoding: 'utf8', maxBuffer: 64 << 20, shell, windowsHide: true, stdio: [stdin, 'pipe', 'pipe'] });
  if (result.error) throw new Error(`${command} could not start: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`${command} ${args.join(' ')} failed (exit ${result.status}): ${(result.stderr || result.stdout).trim().slice(0, 500)}`);
  return result.stdout.trim();
}

function installer(dir) {
  if (existsSync(join(dir, 'pnpm-lock.yaml'))) return ['pnpm', ['install', '--frozen-lockfile']];
  if (existsSync(join(dir, 'yarn.lock'))) return ['yarn', existsSync(join(dir, '.yarnrc.yml')) ? ['install', '--immutable'] : ['install', '--frozen-lockfile']];
  if (existsSync(join(dir, 'bun.lock')) || existsSync(join(dir, 'bun.lockb'))) return ['bun', ['install', '--frozen-lockfile']];
  if (existsSync(join(dir, 'package-lock.json')) || existsSync(join(dir, 'npm-shrinkwrap.json'))) return ['npm', ['ci', '--no-audit', '--no-fund']];
  return ['npm', ['install', '--no-audit', '--no-fund']];
}

const DIR_LINK = WINDOWS ? 'junction' : 'dir';

function ownCode(root, path) {
  const rel = relative(root, path);
  return rel !== '' && !rel.startsWith('..') && !isAbsolute(rel) && !rel.split(sep).includes('node_modules');
}

function entriesOf(folder) {
  const out = [];
  for (const entry of readdirSync(folder, { withFileTypes: true })) {
    const path = join(folder, entry.name);
    if (entry.name.startsWith('@') && entry.isDirectory()) for (const scoped of readdirSync(path)) out.push([`${entry.name}/${scoped}`, join(path, scoped)]);
    else out.push([entry.name, path]);
  }
  return out;
}

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
    else if (statSync(path).isFile()) copyFileSync(path, to);
    else symlinkSync(path, to, DIR_LINK);
  }
}

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
    const input = openSync(archive, 'r');
    try {
      run('tar', ['-xf', '-'], target, { stdin: input });
    } finally {
      closeSync(input);
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
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
