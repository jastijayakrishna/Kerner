import { parentPort, workerData } from 'node:worker_threads';
import { join } from 'node:path';
import { Coordinator } from './coordinator.js';
import { Evidence } from './evidence.js';
import { decide, totals } from './decide.js';
import { versionId } from './identity.js';

// The twin's own thread. Comparing, counting, remembering and deciding all happen here, so none of it can slow or stop the app's
// thread. The app's thread hands over copies of sampled requests; this thread answers with snapshots, and with 'serve' when the
// candidate has earned serving customers or must stop serving them.
const settings = workerData;
const policy = { epsilon: settings.epsilon, minHours: settings.minHours, segments: settings.segments, maxUncovered: settings.maxUncovered };
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
let evidence = null;
let coordinator = null;

const post = (message) => parentPort.postMessage(message);

function snapshot() {
  const data = evidence.data;
  return {
    counts: { ...coordinator.counts, evidenceErrors: evidence.errors },
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

async function onVerdict(verdict) {
  evidence.add(verdict);
  if (crashOnVerdict) process.exit(9);
  if (verdict.outcome === 'different' && serving === 'candidate') {
    await serve('old', `switched back: an unexplained difference (${verdict.kinds.join(', ')}) on ${verdict.route}`);
  } else if (settings.switchOn === 'auto' && router && serving === 'old' && !hold && verdict.outcome !== 'skipped') {
    const decision = decide(evidence.data, policy, Date.now());
    if (decision.ready) await serve('candidate', `switched on: ${decision.needed} clean comparisons in ${policy.segments.join(', ')} and no unexplained difference`);
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
  evidence = await Evidence.open({ file: settings.stateDir ? join(settings.stateDir, 'evidence.json') : null, candidate: candidateId });
  coordinator = new Coordinator({
    ...settings,
    onVerdict,
    onIdle: () => report(true),
    onDisabled: (reason) => {
      if (!closing) post({ type: 'disabled', reason });
    },
  });
  await coordinator.start();
} catch (error) {
  post({ type: 'disabled', reason: `the twin could not start: ${error.message}` });
  coordinator = null;
}
if (coordinator && !coordinator.disabled) {
  parentPort.on('message', receive);
  flusher = setInterval(() => evidence.flush(), FLUSH_MS);
  post({ type: 'ready', candidateId, snapshot: snapshot() });
}
