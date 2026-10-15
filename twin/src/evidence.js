import { createHash } from 'node:crypto';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';

const MAX_SEGMENTS = 200;
const MAX_ROUTES = 500;
const KEEP_UNEXPLAINED = 20;
const FIELD = { same: 'same', stated: 'stated', different: 'different', 'not-compared': 'notCompared', skipped: 'skipped' };

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
const keyOf = (name) => (name === '__proto__' ? '(__proto__)' : name);
const capped = (table, name, max, other) => (Object.hasOwn(table, name) || Object.keys(table).length < max ? name : other);

export class Evidence {
  #file;
  #hashes;
  #dirty = false;
  #flushing = Promise.resolve();
  note = null;
  errors = 0;
  data;

  constructor({ file = null, candidate, old = null, hashes = new Map() }) {
    this.#file = file;
    this.#hashes = hashes;
    this.data = { version: 1, candidate, old, since: 0, segments: {}, unexplained: [], lines: {}, loadLines: [] };
  }

  static async open({ file, candidate, old = null, hashes }) {
    const evidence = new Evidence({ file, candidate, old, hashes });
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
    if (old !== null && body.old !== old) {
      evidence.note = `the evidence file was gathered against another old version (${body.old ?? 'not named'}), not ${old}; started again`;
      return evidence;
    }
    evidence.data = { lines: {}, loadLines: [], ...body };
    return evidence;
  }

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
    if (field === 'same' || field === 'stated' || field === 'different') this.data.since = this.data.since ? Math.min(this.data.since, time) : time;
    if (field === 'different' && this.data.unexplained.length < KEEP_UNEXPLAINED) {
      const { n, at, route: r, segment: s, serving, method, url, kinds } = verdict;
      this.data.unexplained.push({ n, at, route: r, segment: s, serving, method, url, kinds, details: (verdict.details ?? []).slice(0, 5) });
    }
    if (verdict.lines) this.#addLines(field, verdict.lines);
    this.#dirty = true;
  }

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

  setLoadLines(keys) {
    this.data.loadLines = [...new Set([...this.data.loadLines, ...keys])].sort();
    this.#dirty = true;
  }

  snapshot() {
    return structuredClone(this.data);
  }

  flush() {
    const turn = this.#flushing.then(() => this.#write());
    this.#flushing = turn.catch(() => {});
    return turn;
  }

  async #write() {
    if (!this.#file || !this.#dirty) return;
    this.#dirty = false;
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
