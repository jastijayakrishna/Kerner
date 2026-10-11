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

/** The control file's value, or null when it is missing, unreadable or not a control file. */
export async function readControl(file) {
  try {
    const value = JSON.parse(await readFile(file, 'utf8'));
    return value?.version === 1 && typeof value.candidate === 'string' && (value.serving === 'old' || value.serving === 'candidate') ? value : null;
  } catch {
    return null;
  }
}

/** Writes the control file whole: a temporary file, then a rename, so a reader never sees half of it. */
export async function writeControl(file, value) {
  await mkdir(dirname(file), { recursive: true });
  const temp = `${file}.${process.pid}.${Date.now()}.tmp`;
  try {
    await writeFile(temp, `${JSON.stringify(value, null, 2)}\n`);
    await rename(temp, file);
  } catch (error) {
    await rm(temp, { force: true }).catch(() => {});
    throw error;
  }
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
      const control = await readControl(router.#file);
      if (control?.candidate === router.#candidateId) {
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
    const step = this.#queue.then(() => this.#apply(side, reason, persist));
    this.#queue = step.catch(() => false);
    return step;
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
    this.#onSwitch?.(side, this.reason, this.hold);
    if (persist) await this.#persist();
    return true;
  }

  async #persist() {
    if (!this.#file) return;
    try {
      await writeControl(this.#file, { version: 1, candidate: this.#candidateId, serving: this.serving, hold: this.hold, changedAt: new Date().toISOString(), reason: this.reason });
    } catch {
      this.writeErrors++;
    }
  }

  #watch() {
    const file = this.#file;
    // Looked at once a second; it never keeps the process alive. A file that cannot be read leaves serving as it is.
    this.#watcher = async () => {
      const control = await readControl(file);
      if (control?.candidate === this.#candidateId && control.serving !== this.serving) {
        await this.switchTo(control.serving, `the control file: ${control.reason ?? 'changed'}`, { persist: false });
      }
    };
    watchFile(file, { interval: 1000, persistent: false }, this.#watcher);
  }

  /** Stops watching the control file. */
  close() {
    if (this.#watcher) unwatchFile(this.#file, this.#watcher);
    this.#watcher = null;
  }
}
