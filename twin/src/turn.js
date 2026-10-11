import { AsyncLocalStorage } from 'node:async_hooks';

// Whose turn it is inside a ghost: the app's start, or one request. Only the current turn's work gets time (its timers fire) and the
// outside world (its calls are answered and counted). Work an earlier turn left running, a cron tick, a queue's polling loop or a
// retry, waits forever: its timers never fire and its calls never answer. So it can never mix into the request being answered, nor fail
// loudly. Until a first turn begins (a bare seal, as in Kern's own seal tests), everything counts, as before.
const turns = new AsyncLocalStorage();
let begun = false;
let current = null;

/** Starts turn `id`: 'start' for the app's start, the request's number for a request. */
export function begin(id) {
  begun = true;
  current = id;
}

/** Ends the current turn: whatever it left running waits from now on. */
export function end() {
  current = null;
}

/** Runs `fn` inside turn `id`: everything it starts belongs to that turn. */
export const within = (id, fn) => turns.run(id, fn);

/** The turn the running code belongs to: undefined for code no turn started. */
export const owner = () => turns.getStore();

/** Whether code of turn `id` may run now. */
export const isCurrent = (id) => !begun || (id !== undefined && id === current);

/** Whether the running code belongs to the current turn. */
export const mine = () => isCurrent(turns.getStore());

/** No answer, ever: what a call made outside the current turn gets. */
export const never = () => new Promise(() => {});

/** Runs every event of `emitter` inside turn `id`: Node runs a request body's events outside the turn its listener was added in. */
export function keepTurn(emitter, id) {
  const emit = emitter.emit;
  emitter.emit = function (...args) {
    return turns.run(id, () => emit.apply(this, args));
  };
}

/** A turn as it travels in a header ('' for none), and back. */
export const turnText = (id) => (id === undefined ? '' : String(id));
export const turnOf = (text) => (!text ? undefined : /^\d+$/.test(text) ? Number(text) : text);
