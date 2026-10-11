import { Worker } from 'node:worker_threads';
import { current, loadRecord, settleLoad, wrapListener } from './capture.js';
import { recordFetch } from './record.js';
import { ghostEnv } from './ghost-common.js';
import { Router } from './router.js';
import { acceptMatcher } from './manifest.js';
import { createRequire } from 'node:module';
import { ADAPTERS, ADAPTER_NAMES } from './adapters/index.js';
import { spawnSync } from 'node:child_process';

const THREAD = new URL('./twin-thread.js', import.meta.url);
const SIDES = ['old', 'candidate'];
const NO_COUNTS = { sampled: 0, skipped: 0, dropped: 0, same: 0, stated: 0, different: 0, notCompared: 0, restarts: 0, logErrors: 0, evidenceErrors: 0, cases: 0, caseErrors: 0 };

function version(value, name) {
  if (!value || typeof value.module !== 'string' || typeof value.export !== 'string') throw new TypeError(`${name} must be { module: '/absolute/path/to/app.js', export: 'createApp' }`);
  if (value.id !== undefined && (typeof value.id !== 'string' || !value.id)) throw new TypeError(`${name}.id must be a non-empty string`);
  if (value.root !== undefined && (typeof value.root !== 'string' || !value.root)) throw new TypeError(`${name}.root must be the folder the manifest's paths start from`);
  return { module: value.module, export: value.export, ...(value.id ? { id: value.id } : {}), ...(value.root ? { root: value.root } : {}) };
}

function number(value, fallback, min, max, name) {
  const n = value ?? fallback;
  if (typeof n !== 'number' || !(n >= min && n <= max)) throw new TypeError(`${name} must be a number from ${min} to ${max}`);
  return n;
}

function oneOf(value, fallback, choices, name) {
  const v = value ?? fallback;
  if (!choices.includes(v)) throw new TypeError(`${name} must be ${choices.map((c) => `'${c}'`).join(' or ')}`);
  return v;
}

function names(value, fallback, name) {
  const v = value ?? fallback;
  if (!Array.isArray(v) || !v.every((x) => typeof x === 'string' && x.length > 0)) throw new TypeError(`${name} must be a list of non-empty strings`);
  return [...v];
}

function fn(value, name) {
  if (value != null && typeof value !== 'function') throw new TypeError(`${name} must be a function of the request`);
  return value ?? null;
}

/** pgReads: where the twin's thread makes the ghosts' Postgres reads, read-only (a read replica or a read-only role, ideally). */
function connection(value) {
  if (typeof value !== 'string' || !value) throw new TypeError('pgReads must be a Postgres connection string, such as postgres://reader@replica/app');
  return value;
}

function folder(value, name) {
  if (typeof value !== 'string' || !value) throw new TypeError(`${name} must be a folder path`);
  return value;
}

function allowRules(value) {
  if (value == null) return [];
  if (!Array.isArray(value)) throw new TypeError('allow must be a list of { route, paths, kinds }');
  return value.map((rule, i) => {
    if (!rule || typeof rule.route !== 'string' || !rule.route) throw new TypeError(`allow[${i}].route must be a route such as 'GET /users/:id', or '*'`);
    const paths = names(rule.paths, [], `allow[${i}].paths`);
    const kinds = names(rule.kinds, [], `allow[${i}].kinds`);
    if (!paths.length && !kinds.length) throw new TypeError(`allow[${i}] must name paths or kinds`);
    return { route: rule.route, paths, kinds };
  });
}

/** Checks every option and fills in the defaults: a wrong option fails here, before anything starts. */
function settingsOf(options) {
  if (options.stateDir != null && (typeof options.stateDir !== 'string' || !options.stateDir)) throw new TypeError('stateDir must be a folder path');
  if (options.log != null && typeof options.log !== 'string') throw new TypeError('log must be a file path');
  if (options.manifest != null && (typeof options.manifest !== 'string' || !options.manifest)) throw new TypeError('manifest must be the file kern twin plan wrote');
  const settings = {
    old: version(options.old, 'old'),
    candidate: version(options.candidate, 'candidate'),
    sampleRate: number(options.sampleRate, 1, 0, 1, 'sampleRate'),
    timeoutMs: number(options.timeoutMs, 5000, 100, 60000, 'timeoutMs'),
    startMs: number(options.startMs, 20000, 1000, 120000, 'startMs'),
    queueLimit: number(options.queueLimit, 100, 1, 10000, 'queueLimit'),
    maxBodyBytes: number(options.maxBodyBytes, 1 << 20, 1024, 64 << 20, 'maxBodyBytes'),
    ghostMemoryMb: number(options.ghostMemoryMb, 512, 64, 16384, 'ghostMemoryMb'),
    maxRestarts: number(options.maxRestarts, 20, 0, 1000, 'maxRestarts'),
    log: options.log ?? null,
    route: fn(options.route, 'route'),
    segment: fn(options.segment, 'segment'),
    allow: allowRules(options.allow),
    stateDir: options.stateDir ?? null,
    epsilon: number(options.epsilon, 0.001, 0.000001, 0.5, 'epsilon'),
    minHours: number(options.minHours, 24, 0, 2160, 'minHours'),
    segments: names(options.segments, ['*'], 'segments'),
    maxUncovered: number(options.maxUncovered, 0.05, 0, 1, 'maxUncovered'),
    switchOn: oneOf(options.switchOn, 'manual', ['manual', 'auto'], 'switchOn'),
    ghostExecArgv: names(options.ghostExecArgv, [], 'ghostExecArgv'),
    addons: names(options.addons, [], 'addons'),
    manifest: options.manifest ?? null,
    minLineRuns: number(options.minLineRuns, 3, 1, 9, 'minLineRuns'),
    accept: acceptList(options.accept),
    // fetch is always recorded: every ghost replays it.
    adapters: [...new Set(['fetch', ...names(options.adapters, ['fetch'], 'adapters')])],
    realReads: readPatterns(options.realReads),
    overlay: options.overlay === true,
    ghosts: oneOf(options.ghosts, 'thread', ['thread', 'process'], 'ghosts'),
    sandbox: oneOf(options.sandbox, 'none', ['none', 'bwrap'], 'sandbox'),
    cases: options.cases == null ? null : folder(options.cases, 'cases'),
    scrub: { headers: names(options.scrub?.headers, [], 'scrub.headers'), keys: names(options.scrub?.keys, [], 'scrub.keys') },
    humans: { kinds: names(options.humans?.kinds, [], 'humans.kinds'), hosts: names(options.humans?.hosts, [], 'humans.hosts') },
    load: oneOf(options.load, 'none', ['none', 'record'], 'load'),
    pgReads: options.pgReads == null ? null : connection(options.pgReads),
  };
  if (settings.pgReads && !settings.adapters.includes('pg')) throw new TypeError("pgReads needs the 'pg' adapter, whose reads it answers: add 'pg' to adapters");
  if (settings.manifest && (!settings.old.root || !settings.candidate.root)) throw new TypeError("with a manifest, old.root and candidate.root must be the folders its paths start from (each version's checkout)");
  const unknown = settings.adapters.filter((name) => !ADAPTER_NAMES.includes(name));
  if (unknown.length) throw new TypeError(`adapters are ${ADAPTER_NAMES.map((n) => `'${n}'`).join(', ')}; not ${unknown.join(', ')}`);
  if (settings.sandbox === 'bwrap' && settings.ghosts !== 'process') throw new TypeError("the bwrap sandbox holds process ghosts: set ghosts: 'process'");
  if (settings.sandbox === 'bwrap' && process.platform !== 'linux') throw new TypeError('the bwrap sandbox runs on Linux only');
  if (settings.manifest && settings.ghosts === 'process' && settings.sandbox !== 'bwrap') {
    throw new TypeError("measuring changed lines needs the inspector, which Node's permission model refuses: use thread ghosts, or process ghosts in the bwrap sandbox");
  }
  return settings;
}

/** `accept` entries, checked here: 'old:src/x.js:12' or 'new:src/x.js:20-24'. */
function acceptList(value) {
  const list = names(value, [], 'accept');
  acceptMatcher(list);
  return list;
}

/** Allow-listed real reads, checked here: 'GET https://rates.example/*' or 'HEAD https://status.example/ping'. */
function readPatterns(value) {
  const list = names(value, [], 'realReads');
  for (const pattern of list) if (!/^(GET|HEAD) \S+$/.test(pattern)) throw new TypeError(`realReads entries look like 'GET https://rates.example/*', not '${pattern}'`);
  return list;
}

/** What the twin's thread needs: everything but the app's own functions, which cannot cross threads. */
function forThread(settings) {
  const { route, segment, ...rest } = settings;
  return rest;
}

/**
 * The twin as the app's thread sees it: it copies sampled requests and hands them to the twin's own thread, which does the rest, and
 * it routes each request to the version serving now.
 */
class Twin {
  #settings;
  #thread = null;
  #latest = null;
  #disabled = null;
  #candidateId = null;
  #sent = 0;
  #judged = 0;
  #open = 0;
  #dropped = 0;
  #inflight = new Set();
  #waiters = [];
  #router = null;
  #routed = null;
  #switching = Promise.resolve(true);
  #closing = null;
  #loadSent = false;
  #adapters = {};

  constructor(settings) {
    this.#settings = settings;
  }

  async start() {
    if (process.env.KERN_TWIN === 'off') {
      this.#disabled = 'switched off with KERN_TWIN=off';
      return;
    }
    // A sandbox that was asked for and does not work is never quietly dropped: the twin stays off and says why.
    if (this.#settings.sandbox === 'bwrap') {
      const probe = spawnSync('bwrap', ['--unshare-user', '--unshare-net', '--ro-bind', '/', '/', 'true'], { encoding: 'utf8', timeout: 10000 });
      if (probe.status !== 0) {
        this.#disabled = `the bwrap sandbox does not work here: ${String(probe.error?.message ?? probe.stderr ?? '').trim().slice(0, 200)}`;
        return;
      }
    }
    // No Node flags and no NODE_OPTIONS from the app: its preloads have no business in the twin's thread or in the ghosts it starts.
    const thread = new Worker(THREAD, { workerData: forThread(this.#settings), execArgv: [], env: ghostEnv(), resourceLimits: { maxOldGenerationSizeMb: 512 } });
    const first = await new Promise((resolve) => {
      const done = (message) => {
        thread.off('message', onMessage);
        thread.off('error', onError);
        thread.off('exit', onExit);
        resolve(message);
      };
      const onMessage = (message) => {
        if (message?.type === 'ready' || message?.type === 'disabled') done(message);
      };
      const onError = (error) => done({ type: 'disabled', reason: `the twin's thread failed while starting: ${error.message}` });
      const onExit = (code) => done({ type: 'disabled', reason: `the twin's thread stopped while starting (exit ${code})` });
      thread.on('message', onMessage);
      thread.on('error', onError);
      thread.on('exit', onExit);
    });
    if (first.type !== 'ready') {
      this.#disabled = first.reason;
      thread.on('error', () => {});
      await thread.terminate();
      return;
    }
    this.#candidateId = first.candidateId;
    this.#latest = first.snapshot;
    this.#thread = thread;
    // These stay attached for good: without an 'error' listener, a failure in the twin's thread would be thrown in the app's thread.
    thread.on('error', (error) => this.#fail(`the twin's thread failed: ${error.message}`));
    thread.on('exit', (code) => {
      if (!this.#closing) this.#fail(`the twin's thread stopped (exit ${code})`);
    });
    thread.on('message', (message) => this.#receive(message));
    // The twin never keeps the app's process alive. This comes after the 'message' listener: adding the first one refs a worker again.
    thread.unref();
    this.#adapters = this.#record();
  }

  /** Records each listed client library, for both versions' copy of it (one patch when they share a copy). Answers how each went. */
  #record() {
    const report = {};
    for (const name of this.#settings.adapters) {
      if (name === 'fetch') {
        recordFetch(this.#settings.maxBodyBytes);
        report.fetch = 'recording';
        continue;
      }
      const problems = [];
      for (const version of [this.#settings.old, this.#settings.candidate]) {
        try {
          ADAPTERS[name].record({ current, maxBodyBytes: this.#settings.maxBodyBytes, appRequire: createRequire(version.module) });
        } catch (error) {
          problems.push(`${version.module}: ${error.message}`);
        }
      }
      report[name] = problems.length ? `not recorded for ${problems.join('; ')}` : 'recording';
    }
    return report;
  }

  #receive(message) {
    if (message?.type === 'snapshot') {
      this.#latest = message.snapshot;
      this.#judged = Math.max(this.#judged, message.judged);
      this.#settle();
    } else if (message?.type === 'closed') {
      this.#latest = message.snapshot;
    } else if (message?.type === 'serve') {
      this.#switch(message.side, message.reason);
    } else if (message?.type === 'disabled') {
      this.#fail(message.reason);
    }
  }

  /**
   * The twin stops for good and keeps its first reason. The app carries on as if there were no twin; if the new version was serving,
   * the old one serves again, because nothing would switch it back any more.
   */
  #fail(reason) {
    if (this.#disabled) return;
    this.#disabled = reason;
    if (this.#router?.serving === 'candidate') this.#switch('old', `the twin stopped, so the old version serves again: ${reason}`);
    const thread = this.#thread;
    this.#thread = null;
    if (thread && !this.#closing) thread.terminate().catch(() => {});
    this.#settle();
  }

  #switch(side, reason) {
    if (!this.#router) return Promise.resolve(false);
    const step = this.#switching.then(() => this.#router.switchTo(side, reason));
    this.#switching = step.catch(() => false);
    return step;
  }

  /** Whether to copy this request. A full queue sheds load: the customer is served as always, the request is only counted as dropped. */
  #sample() {
    if (this.#disabled || !this.#thread || Math.random() >= this.#settings.sampleRate) return false;
    if (this.#sent - this.#judged + this.#open >= this.#settings.queueLimit) {
      this.#dropped++;
      return false;
    }
    this.#open++;
    return true;
  }

  #deliver(copy) {
    this.#inflight.add(copy);
    copy.then((capture) => this.#post(capture), (error) => this.#fail(`recording failed: ${error?.message ?? error}`)).finally(() => {
      this.#open--;
      this.#inflight.delete(copy);
      this.#settle();
    });
  }

  #post(capture) {
    const thread = this.#thread;
    if (!thread || this.#disabled) return;
    const skip = (reason) => ({ type: 'skip', reason, route: capture.route, segment: capture.segment, serving: capture.serving, start: capture.start });
    try {
      thread.postMessage(capture.skipped ? skip(capture.skipped) : { type: 'capture', capture });
    } catch (error) {
      // A copy that cannot cross to the twin's thread is counted as skipped, never lost.
      thread.postMessage(skip(`a copy that could not be handed over (${error.name})`));
    }
    this.#sent++;
  }

  /** Work the twin's thread still has: copies not yet judged, or known cases it is still checking. */
  #busy() {
    return !this.#disabled && (this.#judged < this.#sent || Boolean(this.#latest?.checking));
  }

  #settle() {
    if (this.#inflight.size || this.#busy()) return;
    for (const resolve of this.#waiters.splice(0)) resolve();
  }

  /** With load 'record', the ghosts start once they have the record of the app's start; the first record sent is the one used. */
  #sendLoad(load) {
    if (this.#settings.load !== 'record' || this.#loadSent) return;
    this.#loadSent = true;
    this.#thread?.postMessage({ type: 'load', load: load ?? { start: Date.now(), seed: 1, io: [] } });
  }

  wrap(listener) {
    // Wrapping the app's own listener, the twin did not see the app start: its ghosts start with no record.
    this.#sendLoad(null);
    const settings = this.#settings;
    return wrapListener(listener, {
      sample: () => this.#sample(),
      maxBodyBytes: settings.maxBodyBytes,
      deliver: (copy) => this.#deliver(copy),
      onError: (error) => this.#fail(`recording failed: ${error?.message ?? error}`),
      describe: (req) => ({ route: settings.route ? settings.route(req) : null, segment: settings.segment ? settings.segment(req) : 'all' }),
    });
  }

  /** The routing listener: it loads the old version in this thread, and records around whichever version serves each request. */
  listener() {
    this.#routed ??= (async () => {
      const open = () => Router.open({
        old: this.#settings.old,
        candidate: this.#settings.candidate,
        candidateId: this.#candidateId,
        stateDir: this.#disabled ? null : this.#settings.stateDir,
        onSwitch: (side, reason, hold) => this.#thread?.postMessage({ type: 'serving', side, reason, hold, router: true }),
      });
      // With load 'record', the old version starts inside a record of its own: every call it makes while starting is written down,
      // and the ghosts start from those answers.
      const record = this.#settings.load === 'record' && this.#thread && !this.#disabled ? loadRecord() : null;
      this.#router = record ? await current.run(record, open) : await open();
      if (record) this.#sendLoad(await settleLoad(record));
      this.#thread?.postMessage({ type: 'serving', side: this.#router.serving, reason: this.#router.reason, hold: this.#router.hold, router: true });
      return this.wrap(this.#router.listener());
    })();
    return this.#routed;
  }

  async serve(side, reason = 'switched by a person') {
    if (!SIDES.includes(side)) throw new TypeError("serve takes 'old' or 'candidate'");
    if (!this.#router) throw new Error("switching needs the twin's own listener: use twin.listener()");
    // With the twin off, nothing would switch the new version back: it may not serve.
    if (side === 'candidate' && this.#disabled) return false;
    return this.#switch(side, reason);
  }

  stats() {
    const latest = this.#latest;
    const counts = { ...NO_COUNTS, ...(latest?.counts ?? {}) };
    return {
      ...counts,
      dropped: counts.dropped + this.#dropped,
      queued: Math.max(0, this.#sent - this.#judged) + this.#open,
      disabled: this.#disabled,
      reasons: { ...(latest?.reasons ?? {}) },
      serving: this.#router?.serving ?? 'old',
      adapters: { ...this.#adapters },
    };
  }

  differences() {
    return [...(this.#latest?.differences ?? [])];
  }

  decision() {
    return this.#latest?.decision ?? null;
  }

  evidence() {
    return this.#latest?.evidence ? structuredClone(this.#latest.evidence) : null;
  }

  /** Every changed line of the manifest with its status: never run, partly run, run, proven, at load or accepted. */
  paths() {
    return [...(this.#latest?.paths ?? [])];
  }

  /**
   * Resolves once every copy handed over so far has been judged, the known cases have been checked, and every switch that caused has
   * been applied.
   */
  async idle() {
    while (this.#inflight.size) await Promise.allSettled([...this.#inflight]);
    if (this.#busy()) {
      // The twin's thread never keeps the process alive by itself, but someone is waiting for it now: an app that has closed its
      // server and awaits idle() would otherwise end before the twin answers, with the work half done.
      const thread = this.#thread;
      thread?.ref();
      try {
        await new Promise((resolve) => this.#waiters.push(resolve));
      } finally {
        thread?.unref();
      }
    }
    await this.#switching;
  }

  /** Writes the evidence and stops the twin's thread and its ghosts. It switches nothing, so a restart keeps what serves. */
  close() {
    this.#closing ??= (async () => {
      this.#router?.close();
      const thread = this.#thread;
      this.#thread = null;
      if (thread) {
        await new Promise((resolve) => {
          const timer = setTimeout(resolve, 10000);
          thread.on('message', (message) => {
            if (message?.type !== 'closed') return;
            clearTimeout(timer);
            resolve();
          });
          thread.postMessage({ type: 'close' });
        });
        await thread.terminate();
      }
      await this.#switching;
      this.#disabled ??= 'closed';
      this.#settle();
    })();
    return this.#closing;
  }
}

/**
 * Starts a twin: the old and the new version of the app run as ghosts in the twin's own thread and answer copies of sampled real
 * requests. Customers get the answers of the version serving, which is the old one until the new one has earned it; if anything goes
 * wrong inside the twin, it switches itself off, says why, and the old version serves.
 */
export async function createTwin(options = {}) {
  const twin = new Twin(settingsOf(options));
  await twin.start();
  return {
    listener: () => twin.listener(),
    wrap: (listener) => twin.wrap(listener),
    stats: () => twin.stats(),
    differences: () => twin.differences(),
    decision: () => twin.decision(),
    evidence: () => twin.evidence(),
    paths: () => twin.paths(),
    serve: (side, reason) => twin.serve(side, reason),
    idle: () => twin.idle(),
    close: () => twin.close(),
  };
}
