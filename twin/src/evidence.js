import { createHash } from 'node:crypto';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';

const MAX_SEGMENTS = 200;
const MAX_ROUTES = 500;
const KEEP_UNEXPLAINED = 20;
const FIELD = { same: 'same', stated: 'stated', different: 'different', 'not-compared': 'notCompared', skipped: 'skipped' };

/**
 * JSON with every object's keys in order: the same content always gives the same text, and so the same checksum. Like JSON, it leaves
 * out keys whose value is undefined and writes undefined in arrays as null, so a file read back gives the checksum it was written with.
 */
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') {
    const keys = Object.keys(value).filter((key) => value[key] !== undefined).sort();
    return `{${keys.map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value ?? null);
}

const checksum = (body) => createHash('sha256').update(canonical(body)).digest('hex');
const zero = () => ({ same: 0, stated: 0, different: 0, notCompared: 0, skipped: 0, first: 0, last: 0 });
// A name chosen by the app becomes a key: '__proto__' would reach the object's prototype instead.
const keyOf = (name) => (name === '__proto__' ? '(__proto__)' : name);
const capped = (table, name, max, other) => (Object.hasOwn(table, name) || Object.keys(table).length < max ? name : other);

/**
 * What the twin has seen of one candidate. Per customer segment and route, it counts how many requests were:
 * - the same;
 * - different in a way the team stated;
 * - different with no explanation;
 * - not compared, or skipped.
 *
 * With a manifest (Phase 3) it also records, per changed line, how often requests ran it and which of its characters clean
 * comparisons ran.
 *
 * Everything is kept in one file, rewritten whole (a temporary file, then a rename) with a checksum. So a crash or a hand edit can
 * never leave counts that look valid but are not.
 */
export class Evidence {
  #file;
  #hashes;
  #dirty = false;
  // Every flush waits here for the ones asked for before it. A flush that found nothing new to write while an earlier write was still
  // in flight would otherwise resolve before that write landed, and the twin's thread, stopped right after its last flush, would lose it.
  #flushing = Promise.resolve();
  note = null;
  errors = 0;
  data;

  constructor({ file = null, candidate, hashes = new Map() }) {
    this.#file = file;
    this.#hashes = hashes;
    this.data = { version: 1, candidate, since: 0, segments: {}, unexplained: [], lines: {}, loadLines: [] };
  }

  /** The evidence for this candidate: what `file` holds if it is intact and for this candidate, else a fresh start with the reason. */
  static async open({ file, candidate, hashes }) {
    const evidence = new Evidence({ file, candidate, hashes });
    if (!file) return evidence;
    let text;
    try {
      text = await readFile(file, 'utf8');
    } catch (error) {
      if (error.code !== 'ENOENT') evidence.note = `the evidence file could not be read (${error.code ?? error.message}); started again`;
      return evidence;
    }
    let stored;
    try {
      stored = JSON.parse(text);
    } catch {
      evidence.note = 'the evidence file was damaged (not JSON); started again';
      return evidence;
    }
    const { sha256, ...body } = stored ?? {};
    if (body.version !== 1 || sha256 !== checksum(body)) {
      evidence.note = 'the evidence file was damaged (its checksum does not match); started again';
      return evidence;
    }
    if (body.candidate !== candidate) {
      evidence.note = `the evidence file was for another candidate (${body.candidate}); started again`;
      return evidence;
    }
    // A file written before Phase 3 has no line evidence yet.
    evidence.data = { lines: {}, loadLines: [], ...body };
    return evidence;
  }

  /** Counts one verdict: { outcome, route, segment, time, kinds, details, lines?, ... }. */
  add(verdict) {
    const field = FIELD[verdict.outcome];
    if (!field) return;
    const segments = this.data.segments;
    const segment = capped(segments, keyOf(String(verdict.segment)), MAX_SEGMENTS, '(other segments)');
    const routes = (segments[segment] ??= {});
    const route = capped(routes, keyOf(String(verdict.route)), MAX_ROUTES, '(other routes)');
    const counts = (routes[route] ??= zero());
    counts[field]++;
    const time = Number(verdict.time) || Date.now();
    counts.first = counts.first ? Math.min(counts.first, time) : time;
    counts.last = Math.max(counts.last, time);
    // minHours counts from the first comparison. Requests that were skipped or not compared say nothing of the candidate: a day of them
    // would otherwise meet a 24-hour minHours after an hour of real comparisons.
    if (field === 'same' || field === 'stated' || field === 'different') this.data.since = this.data.since ? Math.min(this.data.since, time) : time;
    if (field === 'different' && this.data.unexplained.length < KEEP_UNEXPLAINED) {
      const { n, at, route: r, segment: s, serving, method, url, kinds } = verdict;
      this.data.unexplained.push({ n, at, route: r, segment: s, serving, method, url, kinds, details: (verdict.details ?? []).slice(0, 5) });
    }
    if (verdict.lines) this.#addLines(field, verdict.lines);
    this.#dirty = true;
  }

  /**
   * Per changed line, counts the requests that touched it, the requests that ran it fully, and those of them that were clean. Per code
   * character, it counts how many clean comparisons ran it (one digit, 0 to 9), so `a ? b : c` is proven only once both branches ran.
   */
  #addLines(field, lines) {
    const clean = field === 'same' || field === 'stated';
    for (const [key, mask] of Object.entries(lines)) {
      const entry = (this.data.lines[key] ??= { hash: this.#hashes.get(key) ?? null, touched: 0, runs: 0, clean: 0, cover: '' });
      if (entry.cover.length !== mask.length) entry.cover = '0'.repeat(mask.length);
      const full = !mask.includes('0');
      entry.touched++;
      if (full) entry.runs++;
      if (full && clean) entry.clean++;
      if (clean) entry.cover = [...entry.cover].map((digit, i) => (mask[i] === '1' ? String(Math.min(9, Number(digit) + 1)) : digit)).join('');
    }
  }

  /** The changed lines that ran while the versions loaded: they ran before every request. */
  setLoadLines(keys) {
    this.data.loadLines = [...new Set([...this.data.loadLines, ...keys])].sort();
    this.#dirty = true;
  }

  /** A copy of everything counted so far. */
  snapshot() {
    return structuredClone(this.data);
  }

  /**
   * Writes the evidence if it changed: whole, through a temporary file and a rename. A failed write is counted and tried again later.
   * Flushes run one at a time, in the order they were asked for, so one resolves only once every write asked for before it, and its
   * own, are on disk.
   */
  flush() {
    const turn = this.#flushing.then(() => this.#write());
    this.#flushing = turn.catch(() => {});
    return turn;
  }

  async #write() {
    if (!this.#file || !this.#dirty) return;
    this.#dirty = false;
    // The checksum is taken over exactly what JSON keeps, which is what a later open reads back.
    const body = JSON.parse(JSON.stringify(this.data));
    const text = `${JSON.stringify({ ...body, sha256: checksum(body) })}\n`;
    const temp = `${this.#file}.${process.pid}.${Date.now()}.tmp`;
    try {
      await mkdir(dirname(this.#file), { recursive: true });
      await writeFile(temp, text);
      await rename(temp, this.#file);
    } catch {
      this.errors++;
      this.#dirty = true;
      await rm(temp, { force: true }).catch(() => {});
    }
  }
}
