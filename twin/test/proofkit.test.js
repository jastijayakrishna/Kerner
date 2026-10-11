import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { comparedShare, percentile, summarize } from '../proof/kit/stats.mjs';
import { drive, seeded } from '../proof/kit/driver.mjs';
import { report } from '../proof/kit/report.mjs';
import { listen, shopManifest, shopVersion } from './helpers.js';

test('percentiles are exact, by nearest rank, and the share compared counts same, stated and different', () => {
  const values = Array.from({ length: 100 }, (_, i) => 100 - i);
  assert.deepEqual([percentile(values, 50), percentile(values, 99), percentile(values, 100), percentile([], 50)], [50, 99, 100, null]);
  assert.deepEqual(summarize([4, 1, 3, 2]), { count: 4, p50: 2, p90: 4, p99: 4, max: 4 });
  assert.deepEqual(comparedShare({ same: 90, stated: 3, different: 2, notCompared: 4, skipped: 1 }), { compared: 95, judged: 100, share: 0.95 });
});

test('the driver sends the same requests for the same seed at any concurrency, times only the steps, and counts a failing step', async () => {
  const seen = [];
  const server = await listen((req, res) => {
    seen.push(req.url);
    res.writeHead(req.url.startsWith('/missing') ? 404 : 200);
    res.end('ok');
  });
  const scenario = {
    async setup(client) {
      await client.send('/setup');
      return { steps: 0 };
    },
    async step(client, state, random, i) {
      state.steps++;
      if (i === 3) throw new Error('step three breaks');
      const n = Math.floor(random() * 1000);
      await client.send(n % 10 === 0 ? `/missing/${n}` : `/item/${n}`);
    },
  };
  try {
    const one = await drive({ base: server.url, scenario, requests: 40, concurrency: 1, seed: 3 });
    const sentOne = seen.splice(0).filter((url) => url !== '/setup').sort();
    const four = await drive({ base: server.url, scenario, requests: 40, concurrency: 4, seed: 3 });
    const sentFour = seen.splice(0).filter((url) => url !== '/setup').sort();
    assert.deepEqual(sentFour, sentOne, 'the same requests, whatever the order');
    assert.equal(one.ms.length, 39, 'the setup request is not timed, and step three sent nothing');
    assert.equal(Object.values(one.statuses).reduce((a, b) => a + b, 0), 39);
    assert.deepEqual([one.failed, one.failures], [1, ['step 3: step three breaks']]);
    assert.deepEqual([one.state.steps, four.state.steps], [40, 40]);
    assert.notEqual(seeded(3)(), seeded(4)(), 'another seed, other numbers');
  } finally {
    await server.close();
  }
});

test("each step's numbers are spread evenly, even the first one: neighbouring steps must not send alike requests", () => {
  const tenths = (values) => values.reduce((bins, x) => {
    bins[Math.min(9, Math.floor(x * 10))]++;
    return bins;
  }, Array(10).fill(0));
  // The first number of 1,000 neighbouring steps, as the driver seeds them; each tenth should get about 100.
  const first = tenths(Array.from({ length: 1000 }, (_, i) => seeded(7 * 1000003 + i)()));
  assert.ok(first.every((n) => n >= 60 && n <= 140), `first numbers by tenth: ${first.join(' ')}`);
});

test('the report sums each app up from its runs, its differences and the hand-written triage', async () => {
  const root = mkdtempSync(join(tmpdir(), 'kern-proof-'));
  const dir = join(root, 'twin-proof-2026-10-12');
  mkdirSync(dir);
  try {
    const run = (label, p50, p99, twin) => ({
      app: 'demo', label, latency: { count: 10, p50, p90: p99, p99, max: p99 }, rssMb: 80, peakRssMb: 90, twin,
      lines: twin ? { proven: 2, 'never run': 1 } : {},
    });
    const stats = { same: 7, stated: 1, different: 2, notCompared: 1, skipped: 0, compared: 10, judged: 11, share: 0.9091, reasons: { 'not compared: unanswered': 1 } };
    writeFileSync(join(dir, 'demo-off.json'), JSON.stringify(run('off', 1, 2, null)));
    writeFileSync(join(dir, 'demo-sample-1.json'), JSON.stringify(run('sample-1', 1.5, 4, stats)));
    writeFileSync(join(dir, 'demo-sample-1-differences.jsonl'), `${JSON.stringify({ route: 'POST /x', kinds: ['status', 'body'], pulls: [7] })}\n${JSON.stringify({ route: 'POST /x', kinds: ['body', 'status'], pulls: [7] })}\n`);
    writeFileSync(join(dir, 'triage.json'), JSON.stringify({ demo: [{ route: 'POST /x', kinds: ['status', 'body'], count: 2, verdict: 'meant', why: 'the pull request says so' }] }));
    writeFileSync(join(dir, 'gaps.md'), '- demo: nothing to fix.\n');
    const glue = join(dir, 'entry.mjs');
    writeFileSync(glue, '// a comment\nexport const x = 1;\n\nexport const y = 2;\n');
    writeFileSync(join(dir, 'adoption.json'), JSON.stringify({ demo: { files: [glue], options: { load: 'record' }, notes: ['one note'] } }));
    const text = await report(dir);
    assert.match(text, /^# Kern twin: proof on real apps \(2026-10-12\)/);
    for (const line of [
      '| demo | 10 of 11 (90.9%) | 2 | 2 | 0 | 0 | 2 of 3 |',
      '| demo | without the twin | 10 | 1.0 | 2.0 | – | – | 80 (90) |',
      '| demo | all copied | 10 | 1.5 | 4.0 | +0.5 | +2.0 | 80 (90) |',
      '| POST /x | body, status | 2 | #7 | meant | the pull request says so |',
      '- 1 × not compared: unanswered',
      '- demo: nothing to fix.',
      `- Glue: \`${glue}\` (2 lines).`,
      '- one note',
    ]) assert.ok(text.includes(line), `missing: ${line}\n${text}`);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('the proof server and its traffic runner measure an app without the twin and with it', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-proof-run-'));
  const config = join(dir, 'config.json');
  writeFileSync(config, JSON.stringify({
    name: 'three-prs',
    twin: fileURLToPath(new URL('../src/index.js', import.meta.url)),
    old: shopVersion('base'),
    candidate: shopVersion('head'),
    options: { manifest: shopManifest, epsilon: 0.5, minHours: 0, timeoutMs: 3000, log: join(dir, 'three-prs-{label}-differences.jsonl'), cases: join(dir, 'three-prs-{label}-cases') },
  }));
  const kit = (name) => fileURLToPath(new URL(`../proof/kit/${name}`, import.meta.url));
  // What an earlier run with the same label left behind: each run starts from nothing.
  writeFileSync(join(dir, 'three-prs-sample-1-differences.jsonl'), '{"route":"STALE"}\n');
  mkdirSync(join(dir, 'three-prs-sample-1-cases'));
  writeFileSync(join(dir, 'three-prs-sample-1-cases', '1-1-stale.json'), '{}');
  try {
    for (const mode of ['off', 'twin']) {
      const env = { ...process.env, KERN_PROOF_CONFIG: config, KERN_PROOF_MODE: mode, KERN_PROOF_SAMPLE: '1', KERN_PROOF_PORT: '0' };
      const server = spawn(process.execPath, [kit('server.mjs')], { env, stdio: ['ignore', 'pipe', 'inherit'] });
      try {
        const port = await new Promise((resolve, reject) => {
          let text = '';
          server.stdout.on('data', (chunk) => {
            text += chunk;
            const m = /listening on :(\d+)/.exec(text);
            if (m) resolve(Number(m[1]));
          });
          server.once('exit', (code) => reject(new Error(`the proof server stopped (exit ${code})`)));
        });
        const traffic = spawnSync(process.execPath, [kit('traffic.mjs')], {
          env: {
            ...env, KERN_PROOF_URL: `http://127.0.0.1:${port}`, KERN_PROOF_APP: 'three-prs', KERN_PROOF_REQUESTS: '20', KERN_PROOF_CONCURRENCY: '2',
            KERN_PROOF_SCENARIO: fileURLToPath(new URL('./apps/three-prs/scenario.mjs', import.meta.url)), KERN_PROOF_OUT: dir,
          },
          encoding: 'utf8',
          timeout: 120000,
        });
        assert.equal(traffic.status, 0, traffic.stdout + traffic.stderr);
      } finally {
        server.kill();
      }
    }
    const off = JSON.parse(readFileSync(join(dir, 'three-prs-off.json'), 'utf8'));
    const full = JSON.parse(readFileSync(join(dir, 'three-prs-sample-1.json'), 'utf8'));
    assert.deepEqual([off.latency.count, off.twin], [20, null]);
    assert.deepEqual([full.latency.count, full.twin.judged, full.twin.compared], [20, 20, 20], JSON.stringify(full.twin));
    assert.ok(full.differences.some((d) => d.route === 'POST /profile' && d.cause === 102), JSON.stringify(full.differences));
    assert.ok(full.rssMb > 0 && full.lines.proven >= 1, JSON.stringify(full.lines));
    assert.doesNotMatch(readFileSync(join(dir, 'three-prs-sample-1-differences.jsonl'), 'utf8'), /STALE/, "an earlier run's differences are not this run's");
    assert.ok(!readdirSync(join(dir, 'three-prs-sample-1-cases')).includes('1-1-stale.json'), "nor are its cases");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
