import { appendFile } from 'node:fs/promises';
import { compareRuns, faithful } from './compare.js';
import { startGhost } from './ghost.js';

const KEEP = 1000;

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

/**
 * Runs the ghost pair: the old and the new version, each in its own thread. They answer the same requests in the same order and
 * always restart together, so their histories stay identical and a difference can only come from the code.
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
  counts = { sampled: 0, skipped: 0, dropped: 0, same: 0, different: 0, notCompared: 0, restarts: 0, logErrors: 0 };
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
    const settings = { startMs: this.#options.startMs, memoryMb: this.#options.ghostMemoryMb, ghostExecArgv: this.#options.ghostExecArgv, addons: this.#options.addons };
    const named = (label, side) => startGhost(side, settings).catch((error) => {
      throw new Error(`the ${label} version (${side.module}) ${error.message}`);
    });
    const [old, candidate] = await Promise.allSettled([named('old', this.#options.old), named('new', this.#options.candidate)]);
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

  /** Whether a request may be recorded now. A full queue sheds load: the customer is served as always, the request is just not compared. */
  admit() {
    if (this.disabled || !this.#pair) return false;
    if (this.#queue.length >= this.#options.queueLimit) {
      this.counts.dropped++;
      return false;
    }
    return true;
  }

  /** Takes the copy of one served request; requests are compared one at a time, in order. */
  submit(capture) {
    if (this.disabled) return;
    if (capture.skipped) {
      this.counts.skipped++;
      this.#reason(`skipped: ${capture.skipped}`);
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
    const id = ++this.#seq;
    const job = { start: capture.start, seed: capture.seed, request: capture.request, io: capture.io };
    const [old, candidate] = await Promise.all([this.#pair.old.ask(id, job, this.#options.timeoutMs), this.#pair.candidate.ask(id, job, this.#options.timeoutMs)]);
    if (this.disabled) return;
    if (old.dirty || candidate.dirty || this.#dirty) await this.#restart();
    const real = { status: capture.response.status, headers: capture.response.headers, body: asBuffer(capture.response.body), effects: capture.effects, events: [] };
    const oldRun = toRun(old);
    const unfaithful = faithful(real, oldRun);
    const verdict = unfaithful
      ? { outcome: 'not-compared', kinds: ['unfaithful'], details: [{ path: 'ghost', old: '', new: unfaithful }] }
      : compareRuns(oldRun, toRun(candidate));
    if (verdict.outcome === 'same') this.counts.same++;
    else if (verdict.outcome === 'different') this.counts.different++;
    else {
      this.counts.notCompared++;
      this.#reason(`not compared: ${verdict.details[0]?.new ?? verdict.kinds[0]}`);
    }
    if (verdict.outcome === 'different') await this.#remember({ at: new Date(capture.start).toISOString(), method: capture.request.method, url: capture.request.url, ...verdict });
  }

  #reason(text) {
    const key = text.slice(0, 160);
    if (key in this.reasons || Object.keys(this.reasons).length < 50) this.reasons[key] = (this.reasons[key] ?? 0) + 1;
  }

  async #remember(entry) {
    if (this.#found.length < KEEP) this.#found.push(entry);
    if (!this.#options.log) return;
    try {
      await appendFile(this.#options.log, `${JSON.stringify(entry)}\n`);
    } catch {
      this.counts.logErrors++;
    }
  }

  /** Resolves once every submitted request has been compared. */
  idle() {
    if (!this.#busy && !this.#queue.length) return Promise.resolve();
    return new Promise((resolve) => this.#waiters.push(resolve));
  }

  #settle() {
    if (this.#busy || this.#queue.length) return;
    for (const resolve of this.#waiters.splice(0)) resolve();
  }

  /** Switches the twin off for good, keeping the first reason. The app is never affected. */
  disable(reason) {
    if (!this.disabled) this.disabled = reason;
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
