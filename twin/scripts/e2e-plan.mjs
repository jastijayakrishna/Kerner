// End to end for Phase 3: the real `kern twin plan` writes the manifest of the three-pull-request shop, which must say what the
// hand-written test manifest says, and the twin names each difference's pull request from it.
// Needs Node 22, git and a kern binary: node twin/scripts/e2e-plan.mjs --kern <path to kern>
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import http from 'node:http';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../src/index.js';

const at = process.argv.indexOf('--kern');
if (at < 0 || !process.argv[at + 1]) throw new Error('usage: node twin/scripts/e2e-plan.mjs --kern <path to kern>');
const kern = process.argv[at + 1];
const fixture = (...parts) => fileURLToPath(new URL(`../test/apps/three-prs/${parts.join('/')}`, import.meta.url));
const work = mkdtempSync(join(tmpdir(), 'kern-e2e-plan-'));
const repo = join(work, 'repo');
const env = { ...process.env, GIT_AUTHOR_NAME: 't', GIT_AUTHOR_EMAIL: 't@t', GIT_COMMITTER_NAME: 't', GIT_COMMITTER_EMAIL: 't@t', GIT_CONFIG_NOSYSTEM: '1' };
const git = (...args) => execFileSync('git', args, { cwd: repo, env, encoding: 'utf8' }).trim();

try {
  // The repository: the base, then one commit per pull request, each changing one line of the base into the head's.
  mkdirSync(join(repo, 'src'), { recursive: true });
  git('init', '-q');
  const base = readFileSync(fixture('base', 'src', 'app.js'), 'utf8').split('\n');
  const head = readFileSync(fixture('head', 'src', 'app.js'), 'utf8').split('\n');
  writeFileSync(join(repo, 'src', 'app.js'), base.join('\n'));
  git('add', '-A');
  git('commit', '-qm', 'Start the shop');
  const baseSha = git('rev-parse', 'HEAD');
  const now = [...base];
  for (const [line, summary] of [[9, 'Round totals to cents (#101)'], [13, 'Rename the user field (#102)'], [17, 'Tidy the limit check (#103)']]) {
    now[line - 1] = head[line - 1];
    writeFileSync(join(repo, 'src', 'app.js'), now.join('\n'));
    git('commit', '-qam', summary);
  }
  const headSha = git('rev-parse', 'HEAD');
  const manifestFile = join(work, 'manifest.json');
  execFileSync(kern, ['twin', 'plan', repo, baseSha, headSha, manifestFile], { encoding: 'utf8' });

  // kern's manifest says what the hand-written one says, but for commit ids.
  const shape = (m) => m.files.map((f) => ({
    path: f.path, status: f.status, changed: f.changed,
    ...Object.fromEntries(['old', 'new'].map((side) => [side, Object.fromEntries(Object.entries(f[side]).map(([n, e]) => [n, { text: e.text, pull: m.commits[e.commit]?.pull ?? null }]))])),
  }));
  assert.deepEqual(shape(JSON.parse(readFileSync(manifestFile, 'utf8'))), shape(JSON.parse(readFileSync(fixture('manifest.json'), 'utf8'))));

  // The twin, with kern's manifest, names each difference's pull request.
  const side = (version) => ({ module: fixture(version, 'src', 'app.js'), export: 'createApp', root: fixture(version) });
  const twin = await createTwin({ old: side('base'), candidate: side('head'), manifest: manifestFile, timeoutMs: 3000 });
  assert.equal(twin.stats().disabled, null, twin.stats().disabled);
  const server = http.createServer(await twin.listener());
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const post = (path, value) => fetch(`http://127.0.0.1:${server.address().port}${path}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(value) }).then((r) => r.arrayBuffer());
  await post('/total', { items: [{ price: 0.1, count: 3 }] });
  await post('/profile', { name: 'ada', plan: 'pro' });
  await post('/limit', { limits: null });
  await twin.idle();
  assert.deepEqual(Object.fromEntries(twin.differences().map((d) => [d.url, d.cause])), { '/total': 101, '/profile': 102, '/limit': 103 });
  server.closeAllConnections();
  server.close();
  await twin.close();
  console.log('e2e plan: PASSED');
} finally {
  rmSync(work, { recursive: true, force: true });
}
