import http from 'node:http';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { installWorld, resetWorld } from './world.js';
import { installGhostIO, beginJob, endJob, note, addonName, take, wrote } from './ghost-io.js';
import { exactBytes } from './serialize.js';
import { startCoverage, takeLines, targetsOf } from './coverage.js';
import { ADAPTERS } from './adapters/index.js';
import { begin, end, keepTurn, within } from './turn.js';

const tick = setImmediate;
const listen = (server) => new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));

async function openCoverage(plan) {
  const { Session } = await import('node:inspector/promises');
  const session = new Session();
  session.connect();
  await startCoverage(session);
  return { session, side: plan.side, targets: targetsOf(plan), cache: new Map() };
}

function listed(details) {
  const counts = new Map();
  for (const detail of details) counts.set(detail, (counts.get(detail) ?? 0) + 1);
  const shown = [...counts].slice(0, 8).map(([detail, n]) => (n > 1 ? `${detail} (${n} times)` : detail));
  return `${shown.join('; ')}${counts.size > 8 ? `; and ${counts.size - 8} more` : ''}`;
}

function startProblems(events, recorded) {
  const refused = events.filter((event) => event.type === 'denied').map((event) => event.detail);
  const calls = events.filter((event) => event.type.startsWith('unmatched')).map((event) => event.detail);
  let text = '';
  if (refused.length) {
    const addons = [...new Set(refused.map((detail) => /^loading the native add-on (.+)$/.exec(detail)?.[1]).filter(Boolean).map(addonName).filter(Boolean))];
    const allow = addons.length ? `; a native add-on that only computes can be allowed with addons: ${JSON.stringify(addons)}` : '';
    text += ` (at its start the ghost refused it: ${listed(refused)}${allow})`;
  }
  if (calls.length) {
    const list = listed(calls);
    text += recorded
      ? ` (at its start it made calls the record of the app's own start cannot answer: ${list})`
      : ` (at its start it made calls the twin has no answer for: ${list}; with load: 'record' the twin answers them from the app's own start)`;
  }
  return text;
}

export async function runGhost(side, transport) {
  const realRequest = http.request;
  const cap = Number.isFinite(side.maxBodyBytes) ? side.maxBodyBytes : Infinity;
  let handler = null;
  let jobId = 0;
  let askId = 0;
  const asks = new Map();
  const ask = (message) => new Promise((resolve) => {
    const id = ++askId;
    asks.set(id, resolve);
    transport.send({ ...message, id, job: jobId });
  });

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
  for (const name of names) ADAPTERS[name].replay({ appRequire, take, wrote, note, ask, pgReads: Boolean(side.pgReads), ...prepared[name] });
  const coverage = side.coverage ? await openCoverage(side.coverage) : null;
  if (side.load) resetWorld(side.load.start, side.load.seed);
  beginJob(side.load?.io ?? []);
  begin('start');
  const exit = process.exit;
  process.exit = function ghostStartExit(code) {
    throw new Error(`the app ended itself with process.exit(${code ?? ''}) while starting`);
  };
  try {
    await within('start', async () => {
      const app = await import(pathToFileURL(side.module).href);
      const factory = app[side.export];
      if (typeof factory !== 'function') throw new Error(`has no function named ${side.export}`);
      handler = await factory();
      if (typeof handler !== 'function') throw new Error(`${side.export}() must return a request listener: (req, res) => { ... }`);
    });
  } catch (error) {
    throw new Error(`${error?.message ?? error}${startProblems(endJob().events, Boolean(side.load))}`);
  } finally {
    process.exit = exit;
    end();
  }
  endJob();

  let finish = null;
  let tail = false;
  let after = null;
  let answered = null;
  function crashed(error) {
    const message = `uncaught ${error?.name ?? 'error'}: ${String(error?.message ?? error).slice(0, 300)}`;
    transport.send({ type: 'dirty', error: message, job: finish || tail ? jobId : answered });
    if (finish) {
      const done = finish;
      finish = null;
      done({ error: message });
    } else if (tail) after ??= message;
  }
  process.on('uncaughtException', crashed);
  process.on('unhandledRejection', crashed);

  async function run(id, job) {
    jobId = id;
    after = null;
    begin(id);
    resetWorld(job.start, job.seed);
    beginJob(job.io);
    const result = await new Promise((resolve) => {
      finish = resolve;
      const sent = job.request.body ?? new Uint8Array();
      const body = Buffer.from(sent.buffer, sent.byteOffset, sent.byteLength);
      const headers = { ...job.request.headers, 'content-length': String(body.length) };
      const req = realRequest({ host: '127.0.0.1', port, method: job.request.method, path: job.request.url, headers, agent: false }, (res) => {
        let chunks = [];
        let size = 0;
        let hash = null;
        res.on('data', (chunk) => {
          size += chunk.length;
          if (!hash && size > cap) {
            hash = createHash('sha256');
            for (const part of chunks) hash.update(part);
            chunks = [];
          }
          if (hash) hash.update(chunk);
          else chunks.push(chunk);
        });
        res.on('end', () => resolve({
          status: res.statusCode, headers: res.headers,
          ...(hash ? { body: null, large: { size, digest: hash.digest('hex') } } : { body: exactBytes(chunks) }),
        }));
        res.on('error', (error) => resolve({ error: `the answer broke off: ${error.message}` }));
      });
      req.on('error', (error) => resolve({ error: `no answer: ${error.message}` }));
      req.end(body);
    });
    finish = null;
    tail = true;
    await new Promise((resolve) => tick(resolve));
    tail = false;
    const io = endJob();
    end();
    const lines = job.coverage && coverage ? await takeLines(coverage) : undefined;
    answered = id;
    transport.send({
      type: 'result', id,
      result: {
        status: result.status, headers: result.headers, error: result.error, body: result.body ?? null, large: result.large,
        effects: io.effects, writes: io.writes, tooMany: io.tooMany, events: io.events, lines, ...(after ? { after } : {}),
      },
    });
  }

  transport.onMessage((message) => {
    if (message?.type === 'job') run(message.id, message.job);
    else if (message?.type === 'answer') {
      asks.get(message.id)?.(message.answer);
      asks.delete(message.id);
    }
  });
  if (coverage) transport.send({ type: 'load-lines', lines: Object.keys(await takeLines(coverage)) });
  transport.send({ type: 'ready' });
}
