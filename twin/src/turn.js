import { AsyncLocalStorage } from 'node:async_hooks';

const turns = new AsyncLocalStorage();
let begun = false;
let current = null;

export function begin(id) {
  begun = true;
  current = id;
}

export function end() {
  current = null;
}

export const within = (id, fn) => turns.run(id, fn);

export const owner = () => turns.getStore();

export const isCurrent = (id) => !begun || (id !== undefined && id === current);

export const mine = () => isCurrent(turns.getStore());

export const never = () => new Promise(() => {});

export function keepTurn(emitter, id) {
  const emit = emitter.emit;
  emitter.emit = function (...args) {
    return turns.run(id, () => emit.apply(this, args));
  };
}

export const turnText = (id) => (id === undefined ? '' : String(id));
export const turnOf = (text) => (!text ? undefined : /^\d+$/.test(text) ? Number(text) : text);
