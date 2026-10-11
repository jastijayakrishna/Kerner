import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { comparedShare, percentile, summarize } from '../proof/kit/stats.mjs';
import { drive, seeded } from '../proof/kit/driver.mjs';
import { report } from '../proof/kit/report.mjs';
import { listen } from './helpers.js';

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
