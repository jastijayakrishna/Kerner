import { appendFile } from 'node:fs/promises';
import { compareRuns, explain, faithful } from './compare.js';
import { startGhost } from './ghost.js';
import { routeOf } from './route.js';
import { attribute } from './manifest.js';

const KEEP = 1000;
const FIELD = { same: 'same', stated: 'stated', different: 'different', 'not-compared': 'notCompared' };

/** Bytes from another thread as a Buffer, without copying them again. */
const asBuffer = (bytes) => (bytes ? Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength) : Buffer.alloc(0));

function toRun(result) {
  return {
    status: result.status,
    headers: result.headers ?? {},
    body: asBuffer(result.body),
    effects: result.effects ?? [],
    events: result.events ?? [],
    error: result.error,
    timedOut: result.timedOut,
  };
}

/** Where a request belongs: when it came, its route, its customer segment and the version that served it. */
function placeOf(capture) {
  const time = Number(capture.start) || Date.now();
  return {
    time,
    at: new Date(time).toISOString(),
    route: capture.route ?? routeOf(capture.request?.method ?? 'GET', capture.request?.url ?? '/'),
    segment: capture.segment ?? 'all',
    serving: capture.serving === 'candidate' ? 'candidate' : 'old',
  };
}

/**
 * Runs the ghost pair: the old and the new version, each in its own thread. They answer the same requests in the same order and
 * always restart together, so their histories stay identical and a difference can only come from the code. Every request handed in
 * ends as one verdict (same, stated, different, not compared or skipped), handed to `onVerdict`.
 * Options: old, candidate, timeoutMs, startMs, queueLimit, ghostMemoryMb, maxRestarts, log, allow, ghostExecArgv, addons, and the
 * hooks onVerdict(verdict, capture, runs), onIdle() and onDisabled(reason).
 */
export class Coordinator {
  #options;
  #queue = [];
  #busy = false;
  #seq = 0;
  #pair = null;
  #dirty = false;
  #waiters = [];
  #found = [];
  disabled = null;
  counts = { sampled: 0, skipped: 0, dropped: 0, same: 0, stated: 0, different: 0, notCompared: 0, restarts: 0, logErrors: 0 };
  reasons = {};

  constructor(options) {
    this.#options = options;
  }

  async start() {
    try {
      this.#pair = await this.#spawnPair();
    } catch (error) {
      await this.disable(`the ghosts could not start: ${error.message}`);
    }
  }

  async #spawnPair() {
    const o = this.#options;
    const settings = {
      startMs: o.startMs, memoryMb: o.ghostMemoryMb, ghostExecArgv: o.ghostExecArgv, addons: o.addons, maxBodyBytes: o.maxBodyBytes,
      adapters: o.adapters ?? [], overlay: Boolean(o.overlay), realReads: o.realReads ?? [],
    };
    // With a manifest, each ghost measures its own side's changed lines: { old: plan, new: plan }.
    const plans = this.#options.coverage ?? null;
    const named = (label, side, plan) => startGhost(side, { ...settings, coverage: plan }).catch((error) => {
      throw new Error(`the ${label} version (${side.module}) ${error.message}`);
    });
    const [old, candidate] = await Promise.allSettled([named('old', this.#options.old, plans?.old ?? null), named('new', this.#options.candidate, plans?.new ?? null)]);
    if (old.status === 'rejected' || candidate.status === 'rejected') {
      await Promise.allSettled([old, candidate].filter((s) => s.status === 'fulfilled').map((s) => s.value.terminate()));
      throw old.status === 'rejected' ? old.reason : candidate.reason;
    }
    this.#dirty = false;
    for (const ghost of [old.value, candidate.value]) {
      ghost.onDirty(() => {
        this.#dirty = true;
      });
    }
    return { old: old.value, candidate: candidate.value };
  }

  async #restart() {
    this.counts.restarts++;
    const pair = this.#pair;
    this.#pair = null;
    if (pair) await Promise.allSettled([pair.old.terminate(), pair.candidate.terminate()]);
    if (this.disabled) return;
    if (this.counts.restarts > this.#options.maxRestarts) {
      await this.disable(`the ghosts had to restart more than ${this.#options.maxRestarts} times`);
      return;
    }
    try {
      this.#pair = await this.#spawnPair();
    } catch (error) {
      await this.disable(`the ghosts could not restart: ${error.message}`);
    }
  }

  /** Takes one request handed in: a capture to compare, or a skip ({ skipped: reason, ... }) to count. Captures are compared one at a time, in order. */
  submit(capture) {
    if (this.disabled) return;
    if (capture.skipped) {
      this.counts.skipped++;
      this.#reason(`skipped: ${capture.skipped}`);
      const skip = { n: 0, ...placeOf(capture), method: null, url: null, outcome: 'skipped', kinds: ['skipped'], details: [{ kind: 'skipped', path: 'request', old: '', new: String(capture.skipped) }] };
      this.#judge(skip).catch((error) => this.disable(`the twin failed inside: ${error.message}`));
      return;
    }
    if (this.#queue.length >= this.#options.queueLimit) {
      this.counts.dropped++;
      return;
    }
    this.counts.sampled++;
    this.#queue.push(capture);
    this.#pump();
  }

  /** Requests handed in and not yet judged. */
  pending() {
    return this.#queue.length + (this.#busy ? 1 : 0);
  }

  /** The changed lines that ran while each version loaded. */
  loadLines() {
    return this.#pair ? [...this.#pair.old.loadLines, ...this.#pair.candidate.loadLines] : [];
  }

  async #pump() {
    if (this.#busy) return;
    this.#busy = true;
    try {
      while (this.#queue.length && !this.disabled) {
        const capture = this.#queue.shift();
        try {
          await this.#compare(capture);
        } catch (error) {
          await this.disable(`the twin failed inside: ${error.message}`);
        }
      }
    } finally {
      this.#busy = false;
      this.#settle();
    }
  }

  async #compare(capture) {
    if (!this.#pair) return;
    const n = ++this.#seq;
    const job = { start: capture.start, seed: capture.seed, request: capture.request, io: capture.io, coverage: Boolean(this.#options.coverage) };
    const timeout = this.#options.timeoutMs;
    const [old, candidate] = await Promise.all([this.#pair.old.ask(n, job, timeout), this.#pair.candidate.ask(n, job, timeout)]);
    if (this.disabled) return;
    if (old.dirty || candidate.dirty || this.#dirty) await this.#restart();
    const runs = { old: toRun(old), candidate: toRun(candidate) };
    const place = placeOf(capture);
    // The ghost of the version that served is checked against what really happened; then the two ghosts are compared with each other.
    const real = { status: capture.response.status, headers: capture.response.headers, body: asBuffer(capture.response.body), effects: capture.effects, events: [] };
    const unfaithful = faithful(real, runs[place.serving], place.serving);
    const judged = unfaithful
      ? { outcome: 'not-compared', kinds: ['unfaithful'], details: [{ kind: 'unfaithful', path: 'ghost', old: '', new: unfaithful }] }
      : explain(compareRuns(runs.old, runs.candidate), place.route, this.#options.allow ?? []);
    // The changed lines each ghost ran (keys carry their side), and the pull requests behind them.
    const lines = this.#options.coverage ? { ...(old.lines ?? {}), ...(candidate.lines ?? {}) } : null;
    const named = lines && this.#options.manifest ? attribute(Object.keys(lines), this.#options.manifest) : {};
    await this.#judge({ n, ...place, method: capture.request.method, url: capture.request.url, ...judged, ...(lines ? { lines } : {}), ...named }, capture, runs);
  }

  async #judge(verdict, capture = null, runs = null) {
    const field = FIELD[verdict.outcome];
    if (field) this.counts[field]++;
    if (verdict.outcome === 'not-compared') this.#reason(`not compared: ${verdict.details[0]?.new ?? verdict.kinds[0]}`);
    if (verdict.outcome === 'different') await this.#remember(verdict);
    await this.#options.onVerdict?.(verdict, capture, runs);
  }

  #reason(text) {
    const key = text.slice(0, 160);
    if (key in this.reasons || Object.keys(this.reasons).length < 50) this.reasons[key] = (this.reasons[key] ?? 0) + 1;
  }

  async #remember(verdict) {
    if (this.#found.length < KEEP) this.#found.push(verdict);
    if (!this.#options.log) return;
    try {
      await appendFile(this.#options.log, `${JSON.stringify(verdict)}\n`);
    } catch {
      this.counts.logErrors++;
    }
  }

  /** Resolves once every submitted request has been judged. */
  idle() {
    if (!this.#busy && !this.#queue.length) return Promise.resolve();
    return new Promise((resolve) => this.#waiters.push(resolve));
  }

  #settle() {
    if (this.#busy || this.#queue.length) return;
    for (const resolve of this.#waiters.splice(0)) resolve();
    this.#options.onIdle?.();
  }

  /** Switches the twin off for good, keeping the first reason. The app is never affected. */
  disable(reason) {
    if (!this.disabled) {
      this.disabled = reason;
      this.#options.onDisabled?.(reason);
    }
    this.#queue.length = 0;
    const pair = this.#pair;
    this.#pair = null;
    this.#settle();
    return pair ? Promise.allSettled([pair.old.terminate(), pair.candidate.terminate()]) : Promise.resolve();
  }

  close() {
    return this.disable('closed');
  }

  snapshot() {
    return { ...this.counts, queued: this.#queue.length, disabled: this.disabled, reasons: { ...this.reasons } };
  }

  differences() {
    return this.#found.slice();
  }
}
