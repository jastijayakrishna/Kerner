import { parentPort, workerData } from 'node:worker_threads';
import { join } from 'node:path';
import { Coordinator } from './coordinator.js';
import { Evidence } from './evidence.js';
import { decide, pathsOf, totals } from './decide.js';
import { versionId } from './identity.js';
import { acceptMatcher, checkRoots, coveragePlan, loadManifest } from './manifest.js';
import { compareRuns } from './compare.js';
import { writeCase } from './cases.js';
import { needsHuman } from './feedback.js';
import { scrubCapture, scrubber } from './scrub.js';
import { exactBytes } from './serialize.js';

// The twin's own thread. Comparing, counting, remembering and deciding all happen here, so none of it can slow or stop the app's
// thread. The app's thread hands over copies of sampled requests; this thread answers with snapshots, and with 'serve' when the
// candidate has earned serving customers or must stop serving them.
const settings = workerData;
// Only for Kern's own fault tests: the thread ends after its first verdict, as if it had crashed.
const crashOnVerdict = process.env.KERN_TWIN_FAULT === 'thread-exit';
const SNAPSHOT_MS = 250;
const FLUSH_MS = 5000;

let serving = 'old';
let hold = false;
let router = false;
let received = 0;
let lastReport = 0;
let closing = false;
let flusher = null;
let candidateId = null;
let manifest = null;
let policy = null;
let evidence = null;
let coordinator = null;
const CASES_PER_KIND = 5;
const MOST_CASES = 200;
const caseCounts = new Map();
let casesWritten = 0;
let caseErrors = 0;

const post = (message) => parentPort.postMessage(message);

function snapshot() {
  const data = evidence.data;
  return {
    counts: { ...coordinator.counts, evidenceErrors: evidence.errors, cases: casesWritten, caseErrors },
    queued: coordinator.pending(),
    disabled: coordinator.disabled,
    reasons: { ...coordinator.reasons },
    differences: coordinator.differences().slice(0, 100),
    decision: decide(data, policy, Date.now()),
    evidence: {
      candidate: candidateId,
      since: data.since,
      note: evidence.note,
      totals: Object.fromEntries(['*', ...Object.keys(data.segments)].map((segment) => [segment, totals(data, segment)])),
      unexplained: data.unexplained,
    },
    paths: manifest ? pathsOf(data, manifest.lines, policy) : [],
  };
}

/** Tells the app's thread how things stand: at once when everything handed over has been judged, else at most every 250 ms. */
function report(now = false) {
  const pending = coordinator.pending();
  const time = Date.now();
  if (!now && pending && time - lastReport < SNAPSHOT_MS) return;
  lastReport = time;
  post({ type: 'snapshot', snapshot: snapshot(), judged: received - pending });
}

/** Asks the app's thread to switch serving. The evidence behind the switch is written first. */
async function serve(side, reason) {
  serving = side;
  hold = side === 'old';
  await evidence.flush();
  post({ type: 'serve', side, reason });
}

/** A verdict whose shown values are scrubbed: what goes to disk (the evidence's examples, the differences log). */
function redact(verdict) {
  const s = scrubber(settings.scrub ?? {});
  return { ...verdict, url: verdict.url && s.url(verdict.url), details: verdict.details.map((d) => ({ ...d, old: s.text(d.old), new: s.text(d.new) })) };
}

const saved = (run) => ({ status: run.status, headers: run.headers, body: exactBytes([run.body]), effects: run.effects, events: run.events, error: run.error, timedOut: run.timedOut });

/**
 * Turns an unexplained difference into a case: the capture is scrubbed and both ghosts run on the scrubbed copy again.
 * - If the difference shows again, the case holds that copy and both answers.
 * - If not, it depended on a private value: a stub, with no request data, tells a person to look.
 */
async function recordCase(verdict, capture) {
  const group = `${verdict.route} ${verdict.kinds.join(',')}`;
  if (casesWritten >= MOST_CASES || (caseCounts.get(group) ?? 0) >= CASES_PER_KIND) return;
  caseCounts.set(group, (caseCounts.get(group) ?? 0) + 1);
  casesWritten++;
  const { capture: scrubbed, opaque } = scrubCapture(capture, settings.scrub ?? {});
  const runs = await coordinator.rerun(scrubbed);
  const again = runs ? compareRuns(runs.old, runs.candidate) : null;
  const reproduces = again?.outcome === 'different' && again.kinds.some((kind) => verdict.kinds.includes(kind));
  const base = {
    version: 1, candidate: candidateId, n: verdict.n, time: verdict.time, route: verdict.route, segment: verdict.segment,
    adapters: settings.adapters ?? ['fetch'], overlay: Boolean(settings.overlay), kinds: verdict.kinds, lines: verdict.lines ?? {},
    pulls: verdict.pulls ?? [], cause: verdict.cause ?? null, human: needsHuman(verdict, settings.humans), scrubbed: true, opaque, reproduces,
  };
  const entry = reproduces
    ? { ...base, world: { start: scrubbed.start, seed: scrubbed.seed }, request: scrubbed.request, io: scrubbed.io, old: saved(runs.old), new: saved(runs.candidate), verdict: { kinds: again.kinds, details: again.details } }
    : { ...base, why: 'the difference did not show again on the scrubbed copy: it may depend on a private value, so a person must look' };
  try {
    await writeCase(settings.cases, entry);
  } catch {
    caseErrors++;
  }
}

async function onVerdict(verdict, capture) {
  evidence.add(redact(verdict));
  if (verdict.outcome === 'different' && settings.cases && capture) await recordCase(verdict, capture);
  if (crashOnVerdict) process.exit(9);
  if (verdict.outcome === 'different' && serving === 'candidate') {
    await serve('old', `switched back: an unexplained difference (${verdict.kinds.join(', ')}) on ${verdict.route}`);
  } else if (settings.switchOn === 'auto' && router && serving === 'old' && !hold && verdict.outcome !== 'skipped') {
    const decision = decide(evidence.data, policy, Date.now());
    if (decision.ready) await serve('candidate', `switched on: ${decision.needed} clean comparisons in ${policy.segments.join(', ')}, every changed line proven, and no unexplained difference`);
  }
  report();
}

function receive(message) {
  try {
    if (message?.type === 'capture') {
      received++;
      coordinator.submit(message.capture);
    } else if (message?.type === 'skip') {
      received++;
      coordinator.submit({ skipped: message.reason, route: message.route, segment: message.segment, serving: message.serving, start: message.start });
    } else if (message?.type === 'serving') {
      serving = message.side;
      hold = Boolean(message.hold);
      router ||= Boolean(message.router);
    } else if (message?.type === 'close') {
      close();
      return;
    }
    report();
  } catch (error) {
    coordinator.disable(`the twin failed inside: ${error.message}`);
  }
}

async function close() {
  if (closing) return;
  closing = true;
  clearInterval(flusher);
  await coordinator.close();
  await evidence.flush();
  post({ type: 'closed', snapshot: snapshot() });
}

try {
  candidateId = settings.candidate.id ?? (await versionId(settings.candidate.module, settings.candidate.export));
  if (settings.manifest) {
    manifest = await loadManifest(settings.manifest);
    // A manifest describes one change: applied to other code, its line numbers would point at the wrong lines.
    await checkRoots(manifest, { old: settings.old.root, new: settings.candidate.root });
  }
  policy = {
    epsilon: settings.epsilon,
    minHours: settings.minHours,
    segments: settings.segments,
    maxUncovered: settings.maxUncovered,
    minLineRuns: settings.minLineRuns,
    accepted: acceptMatcher(settings.accept ?? []),
    lineKeys: manifest ? [...manifest.lines.keys()] : [],
  };
  const hashes = new Map(manifest ? [...manifest.lines].map(([key, line]) => [key, line.hash]) : []);
  evidence = await Evidence.open({ file: settings.stateDir ? join(settings.stateDir, 'evidence.json') : null, candidate: candidateId, hashes });
  coordinator = new Coordinator({
    ...settings,
    manifest,
    redact,
    coverage: manifest ? { old: coveragePlan(manifest, 'old', settings.old.root), new: coveragePlan(manifest, 'new', settings.candidate.root) } : null,
    onVerdict,
    onIdle: () => report(true),
    onDisabled: (reason) => {
      if (!closing) post({ type: 'disabled', reason });
    },
  });
  await coordinator.start();
  if (manifest && !coordinator.disabled) evidence.setLoadLines(coordinator.loadLines());
} catch (error) {
  post({ type: 'disabled', reason: `the twin could not start: ${error.message}` });
  coordinator = null;
}
if (coordinator && !coordinator.disabled) {
  parentPort.on('message', receive);
  flusher = setInterval(() => evidence.flush(), FLUSH_MS);
  post({ type: 'ready', candidateId, snapshot: snapshot() });
}
