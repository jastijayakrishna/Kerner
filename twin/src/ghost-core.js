import http from 'node:http';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { installWorld, resetWorld } from './world.js';
import { installGhostIO, beginJob, endJob, note, take, wrote } from './ghost-io.js';
import { exactBytes } from './serialize.js';
import { startCoverage, takeLines, targetsOf } from './coverage.js';
import { ADAPTERS } from './adapters/index.js';
import { begin, end, keepTurn, within } from './turn.js';

// The real setImmediate, taken before the world is installed: the ghost's own steps run outside any turn, and must never wait.
const tick = setImmediate;
const listen = (server) => new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));

async function openCoverage(plan) {
  const { Session } = await import('node:inspector/promises');
  const session = new Session();
  session.connect();
  await startCoverage(session);
  return { session, side: plan.side, targets: targetsOf(plan), cache: new Map() };
}

/** The calls a version made while starting that no answer was recorded for: why it could not start, and what would help. */
function unanswered(events, recorded) {
  const calls = events.filter((event) => event.type.startsWith('unmatched')).map((event) => event.detail);
  if (!calls.length) return '';
  const list = calls.slice(0, 3).join('; ');
  return recorded
    ? ` (at its start it made calls the record of the app's own start cannot answer: ${list})`
    : ` (at its start it made calls the twin has no answer for: ${list}; with load: 'record' the twin answers them from the app's own start)`;
}

/**
 * One ghost, whatever carries its messages: a worker thread (worker.js) or a child process (process-ghost-main.js). It works in this
 * order:
 * 1. opens its loopback servers;
 * 2. seals the world;
 * 3. loads one version of the app;
 * 4. answers jobs.
 *
 * `side`: { module, export, addons, coverage, adapters, overlay, realReads, load }.
 * `transport`: { send(message), onMessage(listener), beforeSeal() }.
 */
export async function runGhost(side, transport) {
  const realRequest = http.request;
  let handler = null;
  let jobId = 0;
  let askId = 0;
  const asks = new Map();
  /** Asks the twin's thread something (a real read, Task 5): answered by { type: 'answer', id, answer }. */
  const ask = (message) => new Promise((resolve) => {
    const id = ++askId;
    asks.set(id, resolve);
    transport.send({ ...message, id, job: jobId });
  });

  // Each request belongs to the job being run. Its body's events would otherwise run outside that turn (turn.js).
  const harness = http.createServer((req, res) => {
    const id = jobId;
    keepTurn(req, id);
    keepTurn(res, id);
    return within(id, () => handler(req, res));
  });
  await listen(harness);
  const port = harness.address().port;
  const names = (side.adapters ?? []).filter((name) => ADAPTERS[name]);
  const appRequire = createRequire(side.module);
  // An adapter that needs a loopback server of its own (the node:http replay) opens it now: once sealed, nothing can listen.
  const ports = [port];
  const prepared = {};
  for (const name of names) {
    if (!ADAPTERS[name].prepare) continue;
    prepared[name] = await ADAPTERS[name].prepare({ take, wrote, note, ask, realReads: side.realReads ?? [] });
    ports.push(...prepared[name].ports);
  }
  installWorld();
  transport.beforeSeal?.();
  installGhostIO({ ports, addons: side.addons ?? [], overlay: Boolean(side.overlay), realReads: side.realReads ?? [], ask });
  for (const name of names) ADAPTERS[name].replay({ appRequire, take, wrote, note, ...prepared[name] });
  // The changed lines this version runs, per request, when the twin has a manifest (Phase 3). Coverage starts before the app loads, so
  // every function the app compiles is counted precisely.
  const coverage = side.coverage ? await openCoverage(side.coverage) : null;
  // The app's start: a job of its own, at the real start's clock and seed, its calls answered from the record of that start when the
  // twin has one (load 'record'). A start that fails names the calls it could not have answered.
  if (side.load) resetWorld(side.load.start, side.load.seed);
  beginJob(side.load?.io ?? []);
  begin('start');
  try {
    await within('start', async () => {
      const app = await import(pathToFileURL(side.module).href);
      const factory = app[side.export];
      if (typeof factory !== 'function') throw new Error(`has no function named ${side.export}`);
      handler = await factory();
      if (typeof handler !== 'function') throw new Error(`${side.export}() must return a request listener: (req, res) => { ... }`);
    });
  } catch (error) {
    throw new Error(`${error?.message ?? error}${unanswered(endJob().events, Boolean(side.load))}`);
  } finally {
    end();
  }
  endJob();

  let finish = null;
  function crashed(error) {
    // The app's state may now be broken: the coordinator restarts both ghosts after this request.
    transport.send({ type: 'dirty' });
    const message = `uncaught ${error?.name ?? 'error'}: ${String(error?.message ?? error).slice(0, 300)}`;
    if (finish) {
      const done = finish;
      finish = null;
      done({ error: message });
    }
  }
  process.on('uncaughtException', crashed);
  process.on('unhandledRejection', crashed);

  async function run(id, job) {
    jobId = id;
    begin(id);
    resetWorld(job.start, job.seed);
    beginJob(job.io);
    const result = await new Promise((resolve) => {
      finish = resolve;
      const sent = job.request.body ?? new Uint8Array();
      const body = Buffer.from(sent.buffer, sent.byteOffset, sent.byteLength);
      const headers = { ...job.request.headers, 'content-length': String(body.length) };
      const req = realRequest({ host: '127.0.0.1', port, method: job.request.method, path: job.request.url, headers, agent: false }, (res) => {
        const chunks = [];
        res.on('data', (chunk) => chunks.push(chunk));
        res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: exactBytes(chunks) }));
        res.on('error', (error) => resolve({ error: `the answer broke off: ${error.message}` }));
      });
      req.on('error', (error) => resolve({ error: `no answer: ${error.message}` }));
      req.end(body);
    });
    finish = null;
    // As in the real run's record, calls the app starts right after answering still belong to this request: they get one more tick.
    await new Promise((resolve) => tick(resolve));
    const io = endJob();
    end();
    const lines = job.coverage && coverage ? await takeLines(coverage) : undefined;
    transport.send({
      type: 'result', id,
      result: { status: result.status, headers: result.headers, error: result.error, body: result.body ?? null, effects: io.effects, events: io.events, lines },
    });
  }

  transport.onMessage((message) => {
    if (message?.type === 'job') run(message.id, message.job);
    else if (message?.type === 'answer') {
      asks.get(message.id)?.(message.answer);
      asks.delete(message.id);
    }
  });
  // The changed lines that ran while the app loaded (top-level code): they ran before every request.
  if (coverage) transport.send({ type: 'load-lines', lines: Object.keys(await takeLines(coverage)) });
  transport.send({ type: 'ready' });
}
