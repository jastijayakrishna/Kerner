import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { drive } from './driver.mjs';
import { comparedShare, summarize } from './stats.mjs';

// One proof run's traffic:
// 1. it waits for the app;
// 2. it runs the scenario (KERN_PROOF_SCENARIO exports `scenario`);
// 3. it waits until the twin has judged everything;
// 4. it writes the run's numbers to $KERN_PROOF_OUT/<app>-<label>.json, the label being 'off' or 'sample-<share>'.
const base = process.env.KERN_PROOF_URL ?? 'http://app:8080';
const app = process.env.KERN_PROOF_APP;
const label = process.env.KERN_PROOF_MODE === 'off' ? 'off' : `sample-${process.env.KERN_PROOF_SAMPLE ?? '1'}`;
const requests = Number(process.env.KERN_PROOF_REQUESTS ?? 300);
const concurrency = Number(process.env.KERN_PROOF_CONCURRENCY ?? 2);
const out = process.env.KERN_PROOF_OUT ?? '/out';
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const numbers = async () => (await fetch(`${base}/__proof`)).json();

for (let i = 0; ; i++) {
  try {
    await numbers();
    break;
  } catch {
    if (i > 600) throw new Error(`${app} did not start within 5 minutes`);
    await pause(500);
  }
}
const { scenario } = await import(pathToFileURL(process.env.KERN_PROOF_SCENARIO).href);
const started = Date.now();
const run = await drive({ base, scenario, requests, concurrency, seed: 7 });
const sent = Date.now();
let after = await numbers();
for (let i = 0; after.stats && after.stats.queued > 0 && !after.stats.disabled && i < 7200; i++) {
  await pause(500);
  after = await numbers();
}
const s = after.stats;
// How many changed lines are in each state: never run, partly run, run, proven, at load or accepted (Phase 3).
const lines = {};
for (const p of after.paths) lines[p.status] = (lines[p.status] ?? 0) + 1;
const result = {
  app, label, mode: after.mode, sampleRate: after.sampleRate, requests, concurrency,
  seconds: { traffic: (sent - started) / 1000, judging: (Date.now() - sent) / 1000 },
  latency: summarize(run.ms), statuses: run.statuses, failed: run.failed, failures: run.failures,
  rssMb: Math.round(after.rss / 1048576), peakRssMb: Math.round(after.peakRss / 1048576),
  twin: s ? { ...s, ...comparedShare(s) } : null,
  decision: after.decision, evidence: after.evidence,
  differences: after.differences.map((d) => ({ route: d.route, kinds: d.kinds, cause: d.cause ?? null, pulls: d.pulls ?? [], details: d.details.slice(0, 3) })),
  lines,
};
await mkdir(out, { recursive: true });
await writeFile(join(out, `${app}-${label}.json`), `${JSON.stringify(result, null, 2)}\n`);
console.log(`${app} ${label}: ${run.ms.length} requests, p50 ${result.latency.p50} ms, p99 ${result.latency.p99} ms, ${result.rssMb} MB${s ? `, compared ${result.twin.compared} of ${result.twin.judged}, ${s.different} different` : ''}`);
if (s?.disabled) console.log(`the twin switched itself off: ${s.disabled}`);
if (run.failed) console.log(`${run.failed} steps failed; the first: ${run.failures[0]}`);
