import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const cli = fileURLToPath(new URL('../bin/kern-twin.js', import.meta.url));
const env = { ...process.env, GIT_AUTHOR_NAME: 't', GIT_AUTHOR_EMAIL: 't@t', GIT_COMMITTER_NAME: 't', GIT_COMMITTER_EMAIL: 't@t', GIT_CONFIG_NOSYSTEM: '1' };
const git = (dir, ...args) => execFileSync('git', args, { cwd: dir, env, encoding: 'utf8' }).trim();

function project() {
  const root = mkdtempSync(join(tmpdir(), 'kern-build-'));
  const repo = join(root, 'repo');
  mkdirSync(join(repo, 'src'), { recursive: true });
  git(repo, 'init', '-q');
  writeFileSync(join(repo, 'package.json'), '{"name":"shop","version":"1.0.0","private":true}\n');
  writeFileSync(join(repo, 'package-lock.json'), `${JSON.stringify({ name: 'shop', version: '1.0.0', lockfileVersion: 3, requires: true, packages: { '': { name: 'shop', version: '1.0.0' } } }, null, 2)}\n`);
  writeFileSync(join(repo, '.gitignore'), 'node_modules/\n');
  writeFileSync(join(repo, 'src', 'app.js'), 'export const version = "old";\n');
  git(repo, 'add', '-A');
  git(repo, 'commit', '-qm', 'old');
  const base = git(repo, 'rev-parse', 'HEAD');
  writeFileSync(join(repo, 'src', 'app.js'), 'export const version = "new";\n');
  git(repo, 'commit', '-qam', 'new');
  mkdirSync(join(repo, 'node_modules', 'dep'), { recursive: true });
  writeFileSync(join(repo, 'node_modules', 'dep', 'index.js'), 'module.exports = 1;\n');
  return { root, repo, base };
}

const build = (...args) => JSON.parse(execFileSync(process.execPath, [cli, 'build', ...args], { encoding: 'utf8' }));

test('kern-twin build writes the old version beside the new one, sharing its packages when no package file changed', () => {
  const { root, repo, base } = project();
  try {
    const out = join(root, 'old');
    const result = build('--repo', repo, '--base', base, '--out', out);
    assert.deepEqual([result.status, result.dependencies, result.base], ['BUILT', 'linked', base]);
    assert.equal(readFileSync(join(out, 'src', 'app.js'), 'utf8').replace(/\r\n/g, '\n'), 'export const version = "old";\n');
    assert.equal(realpathSync(join(out, 'node_modules')), realpathSync(join(repo, 'node_modules')));
    assert.equal(existsSync(join(out, '.git')), false);
    const again = spawnSync(process.execPath, [cli, 'build', '--repo', repo, '--base', base, '--out', out], { encoding: 'utf8' });
    assert.equal(again.status, 1);
    assert.match(again.stderr, /already exists/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('when a package file changed, kern-twin build installs the old version\'s own packages from its lockfile', () => {
  const { root, repo, base } = project();
  try {
    writeFileSync(join(repo, 'package.json'), '{"name":"shop","version":"1.0.0","private":true,"description":"now with a description"}\n');
    const out = join(root, 'old');
    const result = build('--repo', repo, '--base', base, '--out', out);
    assert.deepEqual([result.dependencies, result.manager, result.changed], ['installed', 'npm ci --no-audit --no-fund', ['package.json']]);
    const linked = existsSync(join(out, 'node_modules')) && realpathSync(join(out, 'node_modules')) === realpathSync(join(repo, 'node_modules'));
    assert.equal(linked, false, 'the old version has packages of its own');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('in a monorepo the old version loads its own copy of a workspace package, never the new one through a shared link', () => {
  const root = mkdtempSync(join(tmpdir(), 'kern-build-'));
  const repo = join(root, 'repo');
  try {
    mkdirSync(join(repo, 'packages', 'shared'), { recursive: true });
    git(repo, 'init', '-q');
    writeFileSync(join(repo, 'package.json'), '{"name":"mono","private":true,"workspaces":["packages/*"]}\n');
    writeFileSync(join(repo, 'package-lock.json'), '{"name":"mono","lockfileVersion":3,"requires":true,"packages":{}}\n');
    writeFileSync(join(repo, '.gitignore'), 'node_modules/\n');
    writeFileSync(join(repo, 'packages', 'shared', 'package.json'), '{"name":"shared","version":"1.0.0","main":"index.js"}\n');
    writeFileSync(join(repo, 'packages', 'shared', 'index.js'), "module.exports = 'old';\n");
    git(repo, 'add', '-A');
    git(repo, 'commit', '-qm', 'old');
    const base = git(repo, 'rev-parse', 'HEAD');
    writeFileSync(join(repo, 'packages', 'shared', 'index.js'), "module.exports = 'new';\n");
    git(repo, 'commit', '-qam', 'new');
    mkdirSync(join(repo, 'node_modules', 'dep'), { recursive: true });
    writeFileSync(join(repo, 'node_modules', 'dep', 'index.js'), "module.exports = 'dep';\n");
    symlinkSync(join(repo, 'packages', 'shared'), join(repo, 'node_modules', 'shared'), process.platform === 'win32' ? 'junction' : 'dir');
    const out = join(root, 'old');
    assert.equal(build('--repo', repo, '--base', base, '--out', out).dependencies, 'linked');
    const load = createRequire(join(out, 'app.js'));
    assert.equal(load('shared'), 'old', 'the old version runs its own copy of the workspace package');
    assert.equal(load('dep'), 'dep', 'and shares the installed packages');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
