import { unwatchFile, watchFile } from 'node:fs';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { current } from './capture.js';

async function load(version) {
  const app = await import(pathToFileURL(version.module).href);
  const factory = app[version.export];
  if (typeof factory !== 'function') throw new Error(`${version.module} has no function named ${version.export}`);
  const listener = await factory();
  if (typeof listener !== 'function') throw new Error(`${version.export}() must return a request listener: (req, res) => { ... }`);
  return listener;
}

const RETRY_MS = 1000;
const WATCH_MS = 1000;

async function readText(file) {
  try {
    return await readFile(file, 'utf8');
  } catch {
    return null;
  }
}

function parseControl(text) {
  try {
    const value = JSON.parse(text);
    return value?.version === 1 && typeof value.candidate === 'string' && (value.serving === 'old' || value.serving === 'candidate') ? value : null;
  } catch {
    return null;
  }
}

export async function readControl(file) {
  return parseControl(await readText(file));
}

export async function writeControl(file, value) {
  await mkdir(dirname(file), { recursive: true });
  const temp = `${file}.${process.pid}.${Date.now()}.tmp`;
  const text = `${JSON.stringify(value, null, 2)}\n`;
  try {
    await writeFile(temp, text);
    await rename(temp, file);
  } catch (error) {
    await rm(temp, { force: true }).catch(() => {});
    throw error;
  }
  return text;
}

export class Router {
  #versions;
  #listeners = { old: null, candidate: null };
  #file;
  #candidateId;
  #onSwitch;
  #queue = Promise.resolve(true);
  #watcher = null;
  #settle = null;
  #seen = null;
  #unwritten = false;
  #retry = null;
  #closed = false;
  serving = 'old';
  hold = false;
  reason = 'start';
  writeErrors = 0;

  constructor({ old, candidate, candidateId, stateDir, onSwitch }) {
    this.#versions = { old, candidate };
    this.#candidateId = candidateId;
    this.#file = stateDir && candidateId ? join(stateDir, 'control.json') : null;
    this.#onSwitch = onSwitch ?? null;
  }

  static async open(options) {
    const router = new Router(options);
    router.#listeners.old = await load(options.old);
    if (router.#file) {
      const text = await readText(router.#file);
      const control = parseControl(text);
      if (control?.candidate === router.#candidateId) {
        router.#seen = text;
        router.hold = control.hold === true;
        router.reason = String(control.reason ?? 'restored');
        if (control.serving === 'candidate') await router.switchTo('candidate', `restored from the control file: ${control.reason}`, { persist: false });
      } else {
        await router.#persist();
      }
      router.#watch();
    }
    return router;
  }

  listener() {
    return (req, res) => {
      const side = this.serving;
      const record = current.getStore();
      if (record) record.serving = side;
      return this.#listeners[side](req, res);
    };
  }

  switchTo(side, reason, { persist = true } = {}) {
    return this.#enqueue(() => this.#apply(side, reason, persist));
  }

  #enqueue(step) {
    const run = this.#queue.then(step);
    this.#queue = run.catch(() => false);
    return run;
  }

  async #apply(side, reason, persist) {
    if (side !== 'old' && side !== 'candidate') throw new TypeError("serving is 'old' or 'candidate'");
    if (side === 'candidate' && !this.#listeners.candidate) {
      try {
        this.#listeners.candidate = await load(this.#versions.candidate);
      } catch (error) {
        this.hold = true;
        this.reason = `the new version could not load in the app's thread: ${error.message}`.slice(0, 300);
        this.#tell();
        await this.#persist();
        return false;
      }
    }
    this.serving = side;
    this.hold = side === 'old';
    this.reason = String(reason).slice(0, 300);
    this.#tell();
    if (persist) await this.#persist();
    return true;
  }

  #tell() {
    try {
      this.#onSwitch?.(this.serving, this.reason, this.hold);
    } catch {
    }
  }

  async #persist() {
    if (!this.#file) return;
    try {
      this.#seen = await writeControl(this.#file, { version: 1, candidate: this.#candidateId, serving: this.serving, hold: this.hold, changedAt: new Date().toISOString(), reason: this.reason });
      this.#unwritten = false;
    } catch {
      this.writeErrors++;
      this.#unwritten = true;
      this.#writeLater();
    }
  }

  #writeLater() {
    if (this.#retry || this.#closed) return;
    this.#retry = setTimeout(() => {
      this.#retry = null;
      this.#enqueue(async () => {
        if (this.#unwritten && !this.#closed && (await readText(this.#file)) === this.#seen) await this.#persist();
      }).catch(() => {});
    }, RETRY_MS);
    this.#retry.unref();
  }

  #watch() {
    this.#watcher = () => {
      this.#enqueue(() => this.#follow()).catch(() => {});
    };
    watchFile(this.#file, { interval: WATCH_MS, persistent: false }, this.#watcher);
    this.#settle = setTimeout(this.#watcher, WATCH_MS);
    this.#settle.unref();
  }

  async #follow() {
    const text = await readText(this.#file);
    if (text === null || text === this.#seen) return;
    const control = parseControl(text);
    if (control?.candidate !== this.#candidateId) return;
    this.#seen = text;
    this.#unwritten = false;
    if (control.serving !== this.serving) await this.#apply(control.serving, `the control file: ${control.reason ?? 'changed'}`, false);
    else if (this.serving === 'old' && (control.hold === true) !== this.hold) {
      this.hold = control.hold === true;
      this.reason = `the control file: ${control.reason ?? 'changed'}`.slice(0, 300);
      this.#tell();
    }
  }

  close() {
    this.#closed = true;
    if (this.#watcher) unwatchFile(this.#file, this.#watcher);
    this.#watcher = null;
    clearTimeout(this.#settle);
    this.#settle = null;
    clearTimeout(this.#retry);
    this.#retry = null;
  }
}
