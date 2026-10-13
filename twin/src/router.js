import { unwatchFile, watchFile } from 'node:fs';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { current } from './capture.js';

/** Loads one version's request listener in this thread. */
async function load(version) {
  const app = await import(pathToFileURL(version.module).href);
  const factory = app[version.export];
  if (typeof factory !== 'function') throw new Error(`${version.module} has no function named ${version.export}`);
  const listener = await factory();
  if (typeof listener !== 'function') throw new Error(`${version.export}() must return a request listener: (req, res) => { ... }`);
  return listener;
}

// A control file write that failed is tried again this often, until it lands.
const RETRY_MS = 1000;

/** The control file's text, or null when it is missing or cannot be read. */
async function readText(file) {
  try {
    return await readFile(file, 'utf8');
  } catch {
    return null;
  }
}

/** A control file's value from its text, or null when the text is not a control file. */
function parseControl(text) {
  try {
    const value = JSON.parse(text);
    return value?.version === 1 && typeof value.candidate === 'string' && (value.serving === 'old' || value.serving === 'candidate') ? value : null;
  } catch {
    return null;
  }
}

/** The control file's value, or null when it is missing, unreadable or not a control file. */
export async function readControl(file) {
  return parseControl(await readText(file));
}

/** Writes the control file whole: a temporary file, then a rename, so a reader never sees half of it. Answers the text written. */
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

/**
 * Sends each request to the version serving now. The old version serves unless the candidate was switched on: by the twin when it
 * earned it, or by a person (twin.serve(), or the control file). A switch applies from the next request on; switches happen one at a
 * time. Any switch back to the old version holds switching on by itself until a person switches the candidate on again.
 */
export class Router {
  #versions;
  #listeners = { old: null, candidate: null };
  #file;
  #candidateId;
  #onSwitch;
  #queue = Promise.resolve(true);
  #watcher = null;
  // The control file's text as this router last wrote or read it. Seeing it again is no person's switch: after a write of the router's
  // own failed, the file still holds the router's earlier text, which must not switch serving back to what it said.
  #seen = null;
  // Whether serving changed since the last write that landed, and the timer that tries that write again.
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

  /** Loads the old version in this thread and takes up where the control file left off for this candidate. */
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

  /** The one request listener: it sends each request to the version serving now, and notes that side in a sampled request's record. */
  listener() {
    return (req, res) => {
      const side = this.serving;
      const record = current.getStore();
      if (record) record.serving = side;
      return this.#listeners[side](req, res);
    };
  }

  /** Switches serving from the next request on. Answers false when the candidate could not load (serving stays old; `reason` says why). */
  switchTo(side, reason, { persist = true } = {}) {
    return this.#enqueue(() => this.#apply(side, reason, persist));
  }

  /** Runs `step` once every switch and control file check queued before it has finished. The queue itself never rejects. */
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
        this.reason = `the new version could not load in the app's thread: ${error.message}`.slice(0, 300);
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

  /**
   * Tells the twin's thread how serving stands, before any request reaches the new side. A failure to tell it never stops the switch
   * from being written: serving and the control file must always agree.
   */
  #tell() {
    try {
      this.#onSwitch?.(this.serving, this.reason, this.hold);
    } catch {
      // The twin's own callback failed; the switch stands, and is written next.
    }
  }

  /** Writes how serving stands to the control file. A write that fails is counted, and tried again every second until one lands. */
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

  /**
   * Tries the control file write again in a second, in turn with switches and reads, and never keeps the process alive. Until a write
   * lands, a restart reads what the file said before, and a switch back that never reached the disk would let the candidate serve again.
   * A file that changed since the router last wrote or read it holds a person's switch: that is the watcher's to follow, never to cover.
   */
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
    // Looked at once a second; it never keeps the process alive. The file is read in the switch queue, after every switch asked for
    // before and its write: a read of older content still in flight could otherwise undo a switch back made meanwhile. The watcher is
    // called by fs.watchFile, which ignores what it answers, so it must never leave a promise to reject: that would end the app.
    this.#watcher = () => {
      this.#enqueue(() => this.#follow()).catch(() => {});
    };
    watchFile(this.#file, { interval: 1000, persistent: false }, this.#watcher);
  }

  /**
   * Follows a person's switch in the control file. Text the router itself last wrote or read is nothing new, and a file that cannot be
   * read, or is not a control file for this candidate, leaves serving as it is.
   */
  async #follow() {
    const text = await readText(this.#file);
    if (text === null || text === this.#seen) return;
    const control = parseControl(text);
    if (control?.candidate !== this.#candidateId) return;
    this.#seen = text;
    // A person's word supersedes a write of the router's own that has not landed yet.
    this.#unwritten = false;
    if (control.serving !== this.serving) await this.#apply(control.serving, `the control file: ${control.reason ?? 'changed'}`, false);
  }

  /** Stops watching the control file, and trying a failed write again. */
  close() {
    this.#closed = true;
    if (this.#watcher) unwatchFile(this.#file, this.#watcher);
    this.#watcher = null;
    clearTimeout(this.#retry);
    this.#retry = null;
  }
}
