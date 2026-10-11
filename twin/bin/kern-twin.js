#!/usr/bin/env node
// kern-twin: the twin runtime's command line. Every command prints one JSON object; a refusal goes to stderr with exit code 1.
import { join } from 'node:path';
import { readControl, writeControl } from '../src/router.js';
import { Evidence } from '../src/evidence.js';
import { decide, pathsOf, totals } from '../src/decide.js';
import { acceptMatcher, loadManifest } from '../src/manifest.js';

const HELP = `kern-twin - the twin runtime's command line

  kern-twin status --state <dir> [--epsilon 0.001] [--min-hours 24] [--segments '*'] [--max-uncovered 0.05]
                   [--manifest <file> [--min-line-runs 3] [--accept old:src/x.js:12,new:src/x.js:20-24]]
      what the twin has seen of the current candidate, and whether it has earned serving customers
  kern-twin serve old|candidate --state <dir> [--reason <text>]
      switches serving by hand; a running twin follows within a second
`;

function parse(args) {
  const out = { words: [] };
  for (let i = 0; i < args.length; i++) {
    if (args[i].startsWith('--')) out[args[i].slice(2)] = args[++i];
    else out.words.push(args[i]);
  }
  return out;
}

function number(value, fallback, name) {
  const n = Number(value ?? fallback);
  if (!Number.isFinite(n)) throw new Error(`--${name} must be a number`);
  return n;
}

async function control(state) {
  const value = await readControl(join(state, 'control.json'));
  if (!value) throw new Error(`no twin has run with the state folder ${state}`);
  return value;
}

async function status(flags) {
  const now = await control(flags.state);
  const manifest = flags.manifest ? await loadManifest(flags.manifest) : null;
  const hashes = new Map(manifest ? [...manifest.lines].map(([key, line]) => [key, line.hash]) : []);
  const evidence = await Evidence.open({ file: join(flags.state, 'evidence.json'), candidate: now.candidate, hashes });
  const data = evidence.data;
  const policy = {
    epsilon: number(flags.epsilon, 0.001, 'epsilon'),
    minHours: number(flags['min-hours'], 24, 'min-hours'),
    segments: String(flags.segments ?? '*').split(','),
    maxUncovered: number(flags['max-uncovered'], 0.05, 'max-uncovered'),
    minLineRuns: number(flags['min-line-runs'], 3, 'min-line-runs'),
    accepted: acceptMatcher(flags.accept ? String(flags.accept).split(',') : []),
    lineKeys: manifest ? [...manifest.lines.keys()] : [],
  };
  return {
    candidate: now.candidate,
    serving: now.serving,
    hold: now.hold === true,
    reason: now.reason,
    changedAt: now.changedAt,
    note: evidence.note,
    since: data.since ? new Date(data.since).toISOString() : null,
    totals: Object.fromEntries(['*', ...Object.keys(data.segments)].map((segment) => [segment, totals(data, segment)])),
    unexplained: data.unexplained,
    decision: decide(data, policy, Date.now()),
    ...(manifest ? { paths: pathsOf(data, manifest.lines, policy) } : {}),
  };
}

async function serve(flags) {
  const side = flags.words[1];
  if (side !== 'old' && side !== 'candidate') throw new Error("serve takes 'old' or 'candidate'");
  const now = await control(flags.state);
  const next = { ...now, serving: side, hold: side === 'old', changedAt: new Date().toISOString(), reason: flags.reason ?? 'switched by hand with kern-twin serve' };
  await writeControl(join(flags.state, 'control.json'), next);
  return next;
}

const flags = parse(process.argv.slice(2));
const command = { status, serve }[flags.words[0]];
if (!command || !flags.state) {
  process.stdout.write(HELP);
  process.exit(flags.words.length ? 2 : 0);
}
try {
  console.log(JSON.stringify(await command(flags), null, 2));
} catch (error) {
  console.error(JSON.stringify({ status: 'DENIED', reason: error.message }));
  process.exit(1);
}
