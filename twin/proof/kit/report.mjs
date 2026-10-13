import { readdir, readFile, writeFile } from 'node:fs/promises';
import { basename, join } from 'node:path';
import { pathToFileURL } from 'node:url';

// Writes the proof's report from one results folder (reports/twin-proof-<date>/):
// - <app>-off.json, <app>-sample-0.1.json and <app>-sample-1.json: what traffic.mjs wrote for each run;
// - <app>-sample-1-differences.jsonl: every unexplained difference of the run with everything copied (the twin's log, scrubbed);
// - triage.json, adoption.json and gaps.md: written by hand, from the evidence.
// A missing piece shows as '–'. Nothing is guessed.

const MODES = [['off', 'without the twin'], ['sample-0.1', '10% copied'], ['sample-1', 'all copied'], ['sample-1-pgreads', 'all copied + Postgres real reads']];
const DASH = '–';
const num = (x) => (typeof x === 'number' && Number.isFinite(x) ? x.toFixed(1) : DASH);
const pct = (x) => (typeof x === 'number' && Number.isFinite(x) ? `${(x * 100).toFixed(1)}%` : DASH);
const signed = (x) => (typeof x === 'number' && Number.isFinite(x) ? `${x >= 0 ? '+' : ''}${x.toFixed(1)}` : DASH);

// Files that are there but could not be read: the report names them, so nothing drops out of it unseen.
let unreadable = [];

/**
 * A JSON file, or `fallback` when it is not there. A byte order mark (PowerShell 5.1 writes one) is skipped. A file that is there but is
 * not JSON is named in the report.
 */
async function readJson(file, fallback) {
  let text;
  try {
    text = await readFile(file, 'utf8');
  } catch {
    return fallback;
  }
  try {
    return JSON.parse(text.replace(/^\uFEFF/, ''));
  } catch (error) {
    unreadable.push(`${basename(file)} could not be read: ${error.message}`);
    return fallback;
  }
}

async function readText(file) {
  try {
    return await readFile(file, 'utf8');
  } catch {
    return '';
  }
}

/** Lines of code in a glue file: not blank, not a // comment. */
async function codeLines(file) {
  return (await readText(file)).split('\n').filter((line) => line.trim() && !line.trim().startsWith('//')).length;
}

/** Each run, by app and mode. */
async function runsIn(dir) {
  const runs = {};
  for (const name of await readdir(dir)) {
    const m = /^(.+)-(off|sample-[\d.]+(?:-pgreads)?)\.json$/.exec(name);
    if (m) (runs[m[1]] ??= {})[m[2]] = await readJson(join(dir, name), null);
  }
  return runs;
}

/** The headline run's differences (all copied, without pgReads): one object per line of its log. */
async function headlineDiffs(dir, app) {
  return (await readText(join(dir, `${app}-sample-1-differences.jsonl`))).split('\n').filter(Boolean).map((line) => JSON.parse(line));
}

/** Differences grouped by route and kinds, with how many there were and the pull requests behind them. */
function groups(differences) {
  const out = new Map();
  for (const d of differences) {
    const kinds = [...(d.kinds ?? [])].sort();
    const key = `${d.route} ${kinds.join(',')}`;
    const group = out.get(key) ?? { route: d.route, kinds, count: 0, pulls: new Set() };
    group.count++;
    for (const pull of d.pulls ?? []) group.pulls.add(pull);
    out.set(key, group);
  }
  return [...out.values()].sort((a, b) => b.count - a.count);
}

/** The report, as markdown, for the results folder `dir`. */
export async function report(dir) {
  unreadable = [];
  const runs = await runsIn(dir);
  const triage = await readJson(join(dir, 'triage.json'), {});
  const adoption = await readJson(join(dir, 'adoption.json'), {});
  const gaps = (await readText(join(dir, 'gaps.md'))).trim();
  const apps = Object.keys(runs).sort();
  const out = [`# Kern twin: proof on real apps (${basename(dir).replace(/^twin-proof-/, '')})`, ''];
  if (unreadable.length) out.push(...unreadable.map((line) => `- **${line}**`), '');

  out.push('## In short', '', '| App | Compared, all copied | Unexplained differences | Meant | Bugs | False alarms | Changed lines proven |', '|---|---|---|---|---|---|---|');
  for (const app of apps) {
    const t = runs[app]['sample-1']?.twin;
    const verdicts = triage[app] ?? [];
    // Only groups the headline run (no pgReads) actually found are counted here, so "Meant" lines up with "Unexplained differences". A
    // group a pgReads run surfaces (triaged but not in the headline) is shown in the real-reads section instead.
    const headline = groups(await headlineDiffs(dir, app));
    const found = (g) => headline.some((h) => h.route === g.route && [...g.kinds].sort().join(',') === h.kinds.join(','));
    const count = (verdict) => verdicts.filter((g) => g.verdict === verdict && found(g)).reduce((sum, g) => sum + (g.count ?? 0), 0);
    const lines = runs[app]['sample-1']?.lines ?? {};
    // Lines a person accepted (code this process does not serve, say) are shown apart: they are not proven.
    const accepted = lines.accepted ?? 0;
    const total = Object.values(lines).reduce((a, b) => a + b, 0) - accepted;
    const proven = total || accepted ? `${lines.proven ?? 0} of ${total}${accepted ? ` (${accepted} accepted)` : ''}` : DASH;
    out.push(`| ${app} | ${t ? `${t.compared} of ${t.judged} (${pct(t.share)})` : DASH} | ${t ? t.different : DASH} | ${count('meant')} | ${count('bug')} | ${count('false alarm')} | ${proven} |`);
  }

  out.push('', '## What it costs', '', 'Times are measured by the traffic driver, from sending a request to reading its whole answer. Memory is the app process, which holds the ghosts as threads.', '');
  out.push('| App | Run | Requests | p50 ms | p99 ms | p50 change | p99 change | Memory MB (peak) |', '|---|---|---|---|---|---|---|---|');
  for (const app of apps) {
    const base = runs[app].off?.latency;
    for (const [label, name] of MODES) {
      const run = runs[app][label];
      if (!run) continue;
      const l = run.latency;
      const change = (field) => (label === 'off' || !base ? DASH : signed(l[field] - base[field]));
      out.push(`| ${app} | ${name} | ${l.count} | ${num(l.p50)} | ${num(l.p99)} | ${change('p50')} | ${change('p99')} | ${run.rssMb} (${run.peakRssMb}) |`);
    }
  }

  out.push('', '## What it found', '');
  for (const app of apps) {
    const lines = (await readText(join(dir, `${app}-sample-1-differences.jsonl`))).split('\n').filter(Boolean).map((line) => JSON.parse(line));
    const found = groups(lines);
    out.push(`### ${app}`, '');
    if (!found.length) {
      out.push('No unexplained difference.', '');
      continue;
    }
    out.push('| Route | Kinds | Count | Pull requests | Triage | Why |', '|---|---|---|---|---|---|');
    for (const g of found) {
      const t = (triage[app] ?? []).find((entry) => entry.route === g.route && [...entry.kinds].sort().join(',') === g.kinds.join(','));
      out.push(`| ${g.route} | ${g.kinds.join(', ')} | ${g.count} | ${[...g.pulls].map((p) => `#${p}`).join(', ') || DASH} | ${t?.verdict ?? 'not triaged'} | ${t?.why ?? DASH} |`);
    }
    out.push('');
  }

  out.push('## What it could not compare', '');
  for (const app of apps) {
    const reasons = Object.entries(runs[app]['sample-1']?.twin?.reasons ?? {}).sort((a, b) => b[1] - a[1]).slice(0, 8);
    out.push(`### ${app}`, '', reasons.length ? reasons.map(([why, n]) => `- ${n} × ${why}`).join('\n') : 'Nothing.', '');
  }

  // Opt-in Postgres real reads were run a second time, labeled, for the apps that have such a run. The report shows both numbers side by
  // side: a read the new code makes that the old never did is answered read-only, so a request that would be not compared can be compared.
  const withReads = apps.filter((app) => runs[app]['sample-1']?.twin && runs[app]['sample-1-pgreads']?.twin);
  if (withReads.length) {
    out.push('## What Postgres real reads changed', '', 'A second, labeled run turned on opt-in Postgres real reads (off by default). The columns are without them → with them.', '');
    out.push('| App | Compared without | Compared with | Different | Not compared |', '|---|---|---|---|---|');
    for (const app of withReads) {
      const a = runs[app]['sample-1'].twin;
      const b = runs[app]['sample-1-pgreads'].twin;
      const share = (t) => `${t.compared} of ${t.judged} (${pct(t.share)})`;
      out.push(`| ${app} | ${share(a)} | ${share(b)} | ${a.different} → ${b.different} | ${a.notCompared} → ${b.notCompared} |`);
    }
    out.push('');
  }

  out.push('## Gaps found, and what was done', '', gaps || 'None.', '');

  out.push('## What adopting it took', '');
  for (const app of apps) {
    const a = adoption[app];
    if (!a) continue;
    const files = [];
    for (const file of a.files) files.push(`\`${file}\` (${await codeLines(file)} lines)`);
    out.push(`### ${app}`, '', `- Glue: ${files.join(', ')}.`, `- Options: \`${JSON.stringify(a.options)}\`.`, ...(a.notes ?? []).map((note) => `- ${note}`), '');
  }
  return `${out.join('\n')}\n`;
}

// As a program: node twin/proof/kit/report.mjs <results folder> writes <results folder>.md beside it.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const dir = process.argv[2];
  if (!dir) throw new Error('usage: node twin/proof/kit/report.mjs <results folder>');
  const file = `${dir.replace(/[\\/]+$/, '')}.md`;
  await writeFile(file, await report(dir));
  console.log(`wrote ${file}`);
}
