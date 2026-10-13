import { parentPort, workerData } from 'node:worker_threads';
import { readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { Coordinator } from './coordinator.js';
import { Evidence } from './evidence.js';
import { decide, pathsOf, totals } from './decide.js';
import { versionId } from './identity.js';
import { acceptMatcher, checkRoots, coveragePlan, loadManifest } from './manifest.js';
import { compareRuns } from './compare.js';
import { readCase, writeCase } from './cases.js';
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
// Known cases: every one runs on both ghosts before the candidate may switch on.
const known = { pending: 0, failing: [], unchecked: [], stubs: [], unreadable: [] };
let knownNames = [];
// The twin is not idle until the known cases are checked and the switch the last check may allow is made (snapshot.checking).
let checking = false;

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
      known,
    },
    paths: manifest ? pathsOf(data, manifest.lines, policy) : [],
    checking,
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

const saved = (run) => ({ status: run.status, headers: run.headers, body: exactBytes([run.body]), effects: run.effects, writes: run.writes, tooMany: run.tooMany, events: run.events, error: run.error, timedOut: run.timedOut });

/**
 * Turns an unexplained difference into a case: the capture is scrubbed and both ghosts run on the scrubbed copy again.
 * - If the difference shows again, with both versions answering as they did (the same statuses), the case holds that copy and both
 *   answers.
 * - If not, it depended on a private value: a stub, with no request data, tells a person to look. A copy answered with another status
 *   (a sign-in the app can no longer read, say) shows another difference, even when the copies differ too.
 */
async function recordCase(verdict, capture, original) {
  const group = `${verdict.route} ${verdict.kinds.join(',')}`;
  if (casesWritten >= MOST_CASES || (caseCounts.get(group) ?? 0) >= CASES_PER_KIND) return;
  caseCounts.set(group, (caseCounts.get(group) ?? 0) + 1);
  casesWritten++;
  const { capture: scrubbed, opaque } = scrubCapture(capture, settings.scrub ?? {});
  const runs = await coordinator.rerun(scrubbed);
  const again = runs ? compareRuns(runs.old, runs.candidate) : null;
  const sameStatuses = !original || (runs?.old.status === original.old.status && runs?.candidate.status === original.candidate.status);
  const reproduces = sameStatuses && again?.outcome === 'different' && again.kinds.some((kind) => verdict.kinds.includes(kind));
  const why = !sameStatuses && runs
    ? `the scrubbed copy was answered with another status (old ${original.old.status} → ${runs.old.status}, new ${original.candidate.status} → ${runs.candidate.status}): it may depend on a private value, such as a sign-in, so a person must look`
    : 'the difference did not show again on the scrubbed copy: it may depend on a private value, so a person must look';
  const base = {
    version: 1, candidate: candidateId, n: verdict.n, time: verdict.time, route: verdict.route, segment: verdict.segment,
    adapters: settings.adapters ?? ['fetch'], overlay: Boolean(settings.overlay), kinds: verdict.kinds, lines: verdict.lines ?? {},
    pulls: verdict.pulls ?? [], cause: verdict.cause ?? null, human: needsHuman(verdict, settings.humans), scrubbed: true, opaque, reproduces,
  };
  const entry = reproduces
    ? { ...base, world: { start: scrubbed.start, seed: scrubbed.seed }, request: scrubbed.request, io: scrubbed.io, old: saved(runs.old), new: saved(runs.candidate), verdict: { kinds: again.kinds, details: again.details } }
    : { ...base, why };
  try {
    await writeCase(settings.cases, entry);
  } catch {
    caseErrors++;
  }
}

/** Switches the candidate on when switchOn is 'auto', the router serves, nothing holds it, and decide says it has earned it. */
async function maybeSwitchOn() {
  if (settings.switchOn !== 'auto' || !router || serving !== 'old' || hold) return;
  const decision = decide(evidence.data, policy, Date.now());
  if (decision.ready) await serve('candidate', `switched on: ${decision.needed} clean comparisons in ${policy.segments.join(', ')}, every changed line proven, and no unexplained difference`);
}

/** The known cases' file names, newest first (a case's name starts with its time). */
async function knownCaseNames() {
  try {
    return (await readdir(settings.cases)).filter((name) => name.endsWith('.json')).sort().reverse();
  } catch (error) {
    if (error.code !== 'ENOENT') known.unreadable.push(settings.cases);
    return [];
  }
}

/**
 * Runs every known case on both ghosts, newest first, one at a time and in turn with real requests: a known difference is never
 * forgotten.
 * - A case that still shows a difference holds the candidate.
 * - So do a stub, which only a person can judge, and a file that cannot be read.
 * - A case the ghosts can no longer compare (its recording cannot answer a read the candidate now makes, say) is only listed: live
 *   traffic must prove that path.
 */
async function checkKnownCases() {
  try {
    for (const name of knownNames) {
      if (closing || coordinator.disabled) return;
      let value = null;
      try {
        value = await readCase(join(settings.cases, name));
      } catch {
        known.unreadable.push(name);
      }
      if (value && !value.reproduces) known.stubs.push(name);
      if (value?.reproduces) {
        const judged = await coordinator.check({ route: value.route, start: value.world.start, seed: value.world.seed, request: value.request, io: value.io });
        if (judged?.outcome === 'different') known.failing.push(name);
        else if (!judged || judged.outcome === 'not-compared') known.unchecked.push(name);
      }
      known.pending--;
      report();
    }
    if (closing || coordinator.disabled) return;
    // The last check may be all the candidate was waiting for.
    await maybeSwitchOn();
  } finally {
    // Only now is the twin idle. A snapshot saying so before the switch was asked for could let twin.idle() end before it.
    checking = false;
    report(true);
  }
}

async function onVerdict(verdict, capture, runs) {
  evidence.add(redact(verdict));
  if (verdict.outcome === 'different' && settings.cases && capture) await recordCase(verdict, capture, runs);
  if (crashOnVerdict) process.exit(9);
  if (verdict.outcome === 'different' && serving === 'candidate') {
    await serve('old', `switched back: an unexplained difference (${verdict.kinds.join(', ')}) on ${verdict.route}`);
  } else if (verdict.outcome !== 'skipped') {
    await maybeSwitchOn();
  }
  report();
}

/** Starts the ghosts: at once, or with load 'record' once the app's thread sends the record of the app's start. */
async function startGhosts(load) {
  await coordinator.start(load);
  if (manifest && !coordinator.disabled) evidence.setLoadLines(coordinator.loadLines());
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
    } else if (message?.type === 'load') {
      startGhosts(message.load).then(() => report(true), (error) => coordinator.disable(`the ghosts could not start: ${error.message}`));
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
  // The evidence is this candidate against this old version: against another old version (a hotfix deployed as old) it proves nothing.
  const [candidate, old] = await Promise.all([settings.candidate, settings.old].map((version) => version.id ?? versionId(version.module, version.export)));
  candidateId = candidate;
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
    // With their text: a changed line with no code on it has nothing to prove.
    lines: manifest ? manifest.lines : new Map(),
    known,
  };
  const hashes = new Map(manifest ? [...manifest.lines].map(([key, line]) => [key, line.hash]) : []);
  evidence = await Evidence.open({ file: settings.stateDir ? join(settings.stateDir, 'evidence.json') : null, candidate: candidateId, old, hashes });
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
  // With load 'record', the ghosts start once the app's thread sends the record of its start (the 'load' message).
  if (settings.load !== 'record') await startGhosts(null);
  // The known cases are counted before the twin is ready, so no decision is ever taken without them.
  if (settings.cases && !coordinator.disabled) {
    knownNames = await knownCaseNames();
    known.pending = knownNames.length;
    checking = knownNames.length > 0;
  }
} catch (error) {
  post({ type: 'disabled', reason: `the twin could not start: ${error.message}` });
  coordinator = null;
}
if (coordinator && !coordinator.disabled) {
  parentPort.on('message', receive);
  flusher = setInterval(() => evidence.flush(), FLUSH_MS);
  post({ type: 'ready', candidateId, snapshot: snapshot() });
  if (knownNames.length) checkKnownCases().catch((error) => coordinator.disable(`the twin failed inside: ${error.message}`));
}
