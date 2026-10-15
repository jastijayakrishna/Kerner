import { appendFile } from 'node:fs/promises';
import { compareRuns, explain, faithful } from './compare.js';
import { startGhost } from './ghost.js';
import { routeOf } from './route.js';
import { attribute } from './manifest.js';
import { pgReader, realRead } from './real-reads.js';
import { stable } from './keys.js';

const KEEP = 1000;
const LATE_MS = 500;
const FIELD = { same: 'same', stated: 'stated', different: 'different', 'not-compared': 'notCompared' };

const asBuffer = (bytes) => (bytes ? Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength) : Buffer.alloc(0));

export function toRun(result) {
  return {
    status: result.status,
    headers: result.headers ?? {},
    body: asBuffer(result.body),
    large: result.large,
    effects: result.effects ?? [],
    writes: result.writes,
    tooMany: result.tooMany,
    events: result.events ?? [],
    error: result.error,
    timedOut: result.timedOut,
    gone: result.gone,
    after: result.after,
  };
}

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

export class Coordinator {
  #options;
  #queue = [];
  #busy = false;
  #seq = 0;
  #pair = null;
  #dirty = false;
  #waiters = [];
  #found = [];
  #load = null;
  #gate;
  #opened;
  #reads = new Map();
  #pgReader = null;
  #last = null;
  #late = { old: null, candidate: null };
  #asking = false;
  #named = false;
  disabled = null;
  counts = { sampled: 0, skipped: 0, dropped: 0, same: 0, stated: 0, different: 0, notCompared: 0, restarts: 0, logErrors: 0 };
  reasons = {};

  constructor(options) {
    this.#options = options;
    this.#gate = new Promise((resolve) => {
      this.#opened = resolve;
    });
  }

  async start(load = null) {
    this.#load = load;
    try {
      this.#pair = await this.#spawnPair();
    } catch (error) {
      await this.disable(`the ghosts could not start: ${error.message}`);
    } finally {
      this.#opened();
    }
  }

  async #spawnPair() {
    const o = this.#options;
    const settings = {
      startMs: o.startMs, memoryMb: o.ghostMemoryMb, ghostExecArgv: o.ghostExecArgv, addons: o.addons, maxBodyBytes: o.maxBodyBytes,
      adapters: o.adapters ?? [], overlay: Boolean(o.overlay), realReads: o.realReads ?? [], load: this.#load, pgReads: Boolean(o.pgReads),
      onRealRead: (message) => this.#realRead(message),
      kind: o.ghosts ?? 'thread', sandbox: o.sandbox ?? 'none',
    };
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
    this.#named = false;
    const pair = { old: old.value, candidate: candidate.value };
    for (const [side, ghost] of [['old', pair.old], ['candidate', pair.candidate]]) {
      ghost.onDirty((message) => {
        if (this.#pair !== pair) return;
        this.#dirty = true;
        const job = message?.job ?? (this.#asking ? undefined : this.#last?.n);
        if (message?.error && job !== undefined && job === this.#last?.n && !this.#late[side]) {
          this.#late[side] = String(message.error);
          if (!this.#queue.some((item) => item.late)) {
            this.#queue.push({ late: true });
            this.#pump();
          }
        }
      });
    }
    return pair;
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
      if (capture.aborted) this.submit({ ...capture, skipped: 'the request was aborted before its answer finished' });
      else this.counts.dropped++;
      return;
    }
    if (!capture.aborted) this.counts.sampled++;
    this.#queue.push(capture);
    this.#pump();
  }

  pending() {
    return this.#queue.length + (this.#busy ? 1 : 0);
  }

  loadLines() {
    return this.#pair ? [...this.#pair.old.loadLines, ...this.#pair.candidate.loadLines] : [];
  }

  async #ready() {
    await this.#judgeLate();
    const pair = this.#pair;
    const stopped = pair && (pair.old.stopped() || pair.candidate.stopped());
    if (pair && this.#dirty && (this.#named || !stopped)) await this.#restart();
    return Boolean(this.#pair) && !this.disabled;
  }

  async #judgeLate() {
    const last = this.#last;
    if (!last || (!this.#late.old && !this.#late.candidate)) return;
    if (!this.#late.old || !this.#late.candidate) await new Promise((resolve) => setTimeout(resolve, LATE_MS));
    const { old, candidate } = this.#late;
    this.#late = { old: null, candidate: null };
    this.#last = null;
    this.#named = true;
    if (!last.compared || old === candidate) return;
    await this.#judge({ ...last.verdict, outcome: 'different', kinds: ['crash'], details: [{ kind: 'crash', path: 'after the answer', old: old ?? 'none', new: candidate ?? 'none' }] });
  }

  async rerun(capture) {
    if (!(await this.#ready())) return null;
    const n = ++this.#seq;
    const job = { start: capture.start, seed: capture.seed, request: capture.request, io: capture.io, coverage: Boolean(this.#options.coverage) };
    const timeout = this.#options.timeoutMs;
    this.#asking = true;
    const [old, candidate] = await Promise.all([this.#pair.old.ask(n, job, timeout), this.#pair.candidate.ask(n, job, timeout)]).finally(() => {
      this.#asking = false;
    });
    this.#reads.clear();
    if (old.dirty || candidate.dirty || this.#dirty) await this.#restart();
    return { old: toRun(old), candidate: toRun(candidate) };
  }

  check(capture) {
    if (this.disabled) return Promise.resolve(null);
    return new Promise((resolve) => {
      this.#queue.push({ ...capture, check: resolve });
      this.#pump();
    });
  }

  async #recheck(capture) {
    let judged = null;
    try {
      const runs = await this.rerun(capture);
      if (runs && !this.disabled) judged = explain(compareRuns(runs.old, runs.candidate), capture.route, this.#options.allow ?? []);
    } finally {
      capture.check(judged);
    }
  }

  #realRead(message) {
    if (message.kind === 'pg') {
      const key = `${message.job} PG ${message.text} ${stable(message.values)}`;
      if (!this.#reads.has(key)) {
        this.#pgReader ??= this.#options.pgReads ? pgReader(this.#options.pgReads, this.#options.old.module) : null;
        this.#reads.set(key, this.#pgReader ? this.#pgReader.read(message.text, message.values) : Promise.resolve({ error: 'no Postgres real reads here' }));
      }
      return this.#reads.get(key);
    }
    const key = `${message.job} ${message.method} ${message.url}`;
    if (!this.#reads.has(key)) this.#reads.set(key, realRead(message, this.#options.realReads ?? [], this.#options.maxBodyBytes));
    return this.#reads.get(key);
  }

  async #pump() {
    if (this.#busy) return;
    this.#busy = true;
    try {
      await this.#gate;
      while (this.#queue.length && !this.disabled) {
        const capture = this.#queue.shift();
        try {
          await (capture.late ? this.#judgeLate() : capture.check ? this.#recheck(capture) : this.#compare(capture));
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
    if (!(await this.#ready())) return;
    const n = ++this.#seq;
    const job = { start: capture.start, seed: capture.seed, request: capture.request, io: capture.io, coverage: Boolean(this.#options.coverage) };
    const timeout = this.#options.timeoutMs;
    this.#asking = true;
    const [old, candidate] = await Promise.all([this.#pair.old.ask(n, job, timeout), this.#pair.candidate.ask(n, job, timeout)]).finally(() => {
      this.#asking = false;
    });
    this.#reads.clear();
    if (this.disabled) return;
    const place = placeOf(capture);
    this.#last = { n, verdict: { n, ...place, method: capture.request.method, url: capture.request.url }, compared: false };
    if (old.dirty || candidate.dirty || this.#dirty) await this.#restart();
    const runs = { old: toRun(old), candidate: toRun(candidate) };
    if (capture.aborted) {
      await this.#judgeAborted(n, place, capture, runs);
      return;
    }
    const real = { status: capture.response.status, headers: capture.response.headers, body: asBuffer(capture.response.body), effects: capture.effects, events: [] };
    const unfaithful = faithful(real, runs[place.serving], place.serving);
    const judged = unfaithful
      ? { outcome: 'not-compared', kinds: ['unfaithful'], details: [{ kind: 'unfaithful', path: 'ghost', old: '', new: unfaithful }] }
      : explain(compareRuns(runs.old, runs.candidate), place.route, this.#options.allow ?? []);
    const lines = this.#options.coverage ? { ...(old.lines ?? {}), ...(candidate.lines ?? {}) } : null;
    const named = lines && this.#options.manifest ? attribute(Object.keys(lines), this.#options.manifest) : {};
    if (this.#last?.n === n) this.#last.compared = ['same', 'stated', 'different'].includes(judged.outcome);
    await this.#judge({ n, ...place, method: capture.request.method, url: capture.request.url, ...judged, ...(lines ? { lines } : {}), ...named }, capture, runs);
  }

  async #judgeAborted(n, place, capture, runs) {
    const verdict = explain(compareRuns(runs.old, runs.candidate), place.route, this.#options.allow ?? []);
    const where = { n, ...place, method: capture.request.method, url: capture.request.url };
    if (verdict.outcome === 'different' && verdict.kinds.includes('hang') && !runs.old.error) {
      await this.#judge({ ...where, ...verdict }, capture, runs);
      return;
    }
    const reason = 'the request was aborted before its answer finished';
    this.counts.skipped++;
    this.#reason(`skipped: ${reason}`);
    await this.#judge({ ...where, outcome: 'skipped', kinds: ['skipped'], details: [{ kind: 'skipped', path: 'request', old: '', new: reason }] });
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
      await appendFile(this.#options.log, `${JSON.stringify(this.#options.redact ? this.#options.redact(verdict) : verdict)}\n`);
    } catch {
      this.counts.logErrors++;
    }
  }

  idle() {
    if (!this.#busy && !this.#queue.length) return Promise.resolve();
    return new Promise((resolve) => this.#waiters.push(resolve));
  }

  #settle() {
    if (this.#busy || this.#queue.length) return;
    for (const resolve of this.#waiters.splice(0)) resolve();
    this.#options.onIdle?.();
  }

  disable(reason) {
    this.#opened();
    if (!this.disabled) {
      this.disabled = reason;
      this.#options.onDisabled?.(reason);
    }
    for (const item of this.#queue.splice(0)) item.check?.(null);
    const pair = this.#pair;
    this.#pair = null;
    this.#settle();
    const reader = this.#pgReader;
    this.#pgReader = null;
    return Promise.allSettled([...(pair ? [pair.old.terminate(), pair.candidate.terminate()] : []), reader?.close()]);
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
