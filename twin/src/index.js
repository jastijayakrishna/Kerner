import { Worker } from 'node:worker_threads';
import { wrapListener } from './capture.js';
import { recordFetch } from './record.js';
import { ghostEnv } from './ghost.js';

const THREAD = new URL('./twin-thread.js', import.meta.url);
const NO_COUNTS = { sampled: 0, skipped: 0, dropped: 0, same: 0, stated: 0, different: 0, notCompared: 0, restarts: 0, logErrors: 0, evidenceErrors: 0 };

function version(value, name) {
  if (!value || typeof value.module !== 'string' || typeof value.export !== 'string') throw new TypeError(`${name} must be { module: '/absolute/path/to/app.js', export: 'createApp' }`);
  if (value.id !== undefined && (typeof value.id !== 'string' || !value.id)) throw new TypeError(`${name}.id must be a non-empty string`);
  return { module: value.module, export: value.export, ...(value.id ? { id: value.id } : {}) };
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
  return {
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
  };
}

/** What the twin's thread needs: everything but the app's own functions, which cannot cross threads. */
function forThread(settings) {
  const { route, segment, ...rest } = settings;
  return rest;
}

/** The twin as the app's thread sees it: it copies sampled requests and hands them to the twin's own thread, which does the rest. */
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
  #closing = null;

  constructor(settings) {
    this.#settings = settings;
  }

  async start() {
    if (process.env.KERN_TWIN === 'off') {
      this.#disabled = 'switched off with KERN_TWIN=off';
      return;
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
    recordFetch(this.#settings.maxBodyBytes);
  }

  #receive(message) {
    if (message?.type === 'snapshot') {
      this.#latest = message.snapshot;
      this.#judged = Math.max(this.#judged, message.judged);
      this.#settle();
    } else if (message?.type === 'closed') {
      this.#latest = message.snapshot;
    } else if (message?.type === 'disabled') {
      this.#fail(message.reason);
    }
  }

  /** The twin stops for good and keeps its first reason. The app carries on as if there were no twin. */
  #fail(reason) {
    if (this.#disabled) return;
    this.#disabled = reason;
    const thread = this.#thread;
    this.#thread = null;
    if (thread && !this.#closing) thread.terminate().catch(() => {});
    this.#settle();
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

  #settle() {
    if (this.#inflight.size || (!this.#disabled && this.#judged < this.#sent)) return;
    for (const resolve of this.#waiters.splice(0)) resolve();
  }

  wrap(listener) {
    const settings = this.#settings;
    return wrapListener(listener, {
      sample: () => this.#sample(),
      maxBodyBytes: settings.maxBodyBytes,
      deliver: (copy) => this.#deliver(copy),
      onError: (error) => this.#fail(`recording failed: ${error?.message ?? error}`),
      describe: (req) => ({ route: settings.route ? settings.route(req) : null, segment: settings.segment ? settings.segment(req) : 'all' }),
    });
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
      serving: 'old',
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

  async idle() {
    while (this.#inflight.size) await Promise.allSettled([...this.#inflight]);
    if (!this.#disabled && this.#judged < this.#sent) await new Promise((resolve) => this.#waiters.push(resolve));
  }

  close() {
    this.#closing ??= (async () => {
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
      this.#disabled ??= 'closed';
      this.#settle();
    })();
    return this.#closing;
  }
}

/**
 * Starts a twin: the old and the new version of the app run as ghosts in the twin's own thread and answer copies of sampled real
 * requests. The twin never changes what a customer gets; if anything goes wrong inside it, it switches itself off and says why.
 */
export async function createTwin(options = {}) {
  const twin = new Twin(settingsOf(options));
  await twin.start();
  return {
    wrap: (listener) => twin.wrap(listener),
    stats: () => twin.stats(),
    differences: () => twin.differences(),
    decision: () => twin.decision(),
    evidence: () => twin.evidence(),
    idle: () => twin.idle(),
    close: () => twin.close(),
  };
}
