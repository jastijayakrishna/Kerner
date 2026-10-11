import crypto from 'node:crypto';
import { syncBuiltinESMExports } from 'node:module';
import timers from 'node:timers';
import timersPromises from 'node:timers/promises';
import { promisify } from 'node:util';
import { isCurrent, owner } from './turn.js';

// Time and chance inside a ghost. Both ghosts of one request see the same clock and the same random numbers, so time and
// randomness can never show up as a difference. The clock does not move during a request.
let now = 0;
let state = 1;

function random() {
  state = (state + 0x6d2b79f5) >>> 0;
  let t = state;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

/** Sets the clock to the moment the real request arrived, and seeds the random numbers for this request. */
export function resetWorld(time, seed) {
  now = time;
  state = seed >>> 0;
}

/** Replaces the clock and every source of randomness in this thread. Call once, before the app loads. */
export function installWorld() {
  const Real = Date;
  function GhostDate(...args) {
    if (!new.target) return new Real(now).toString();
    return Reflect.construct(Real, args.length ? args : [now], new.target);
  }
  GhostDate.prototype = Real.prototype;
  GhostDate.now = () => now;
  GhostDate.UTC = Real.UTC;
  GhostDate.parse = Real.parse;
  globalThis.Date = GhostDate;
  Math.random = random;
  const bytes = (n) => {
    const b = Buffer.alloc(n);
    for (let i = 0; i < n; i++) b[i] = Math.floor(random() * 256);
    return b;
  };
  const fill = (view) => {
    new Uint8Array(view.buffer, view.byteOffset, view.byteLength).set(bytes(view.byteLength));
    return view;
  };
  const uuid = () => {
    const b = bytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    const h = b.toString('hex');
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
  };
  // Some of these are getters on Node's own objects: define them over, never assign.
  const put = (owner, name, value) => {
    try {
      Object.defineProperty(owner, name, { value, writable: true, configurable: true, enumerable: true });
    } catch {
      // A frozen object keeps its own; the old and the new ghost then still behave alike.
    }
  };
  put(crypto, 'randomBytes', (n, callback) => {
    const b = bytes(n);
    if (typeof callback === 'function') {
      process.nextTick(callback, null, b);
      return undefined;
    }
    return b;
  });
  put(crypto, 'randomUUID', uuid);
  put(crypto, 'getRandomValues', fill);
  // Id generators (nanoid, say) fill bytes in place: an ArrayBuffer, a typed array or a DataView, all of it or offset and size.
  const fillRange = (target, offset = 0, size = undefined) => {
    const view = target instanceof ArrayBuffer ? new Uint8Array(target) : new Uint8Array(target.buffer, target.byteOffset, target.byteLength);
    view.set(bytes(size ?? view.length - offset), offset);
    return target;
  };
  put(crypto, 'randomFillSync', (target, offset, size) => fillRange(target, offset, size));
  put(crypto, 'randomFill', (target, ...rest) => {
    const done = rest.findLast((arg) => typeof arg === 'function');
    const [offset, size] = rest.filter((arg) => typeof arg === 'number');
    fillRange(target, offset, size);
    process.nextTick(done, null, target);
  });
  put(crypto, 'randomInt', (a, b, callback) => {
    const [low, high] = b === undefined || typeof b === 'function' ? [0, a] : [a, b];
    const value = low + Math.floor(random() * (high - low));
    const done = typeof b === 'function' ? b : callback;
    if (typeof done === 'function') {
      process.nextTick(done, null, value);
      return undefined;
    }
    return value;
  });
  if (globalThis.crypto) {
    put(globalThis.crypto, 'randomUUID', uuid);
    put(globalThis.crypto, 'getRandomValues', fill);
  }
  put(performance, 'now', () => 0);
  const hrtime = (previous) => {
    const ns = BigInt(now) * 1000000n;
    const t = [Number(ns / 1000000000n), Number(ns % 1000000000n)];
    if (!previous) return t;
    let seconds = t[0] - previous[0];
    let nanos = t[1] - previous[1];
    if (nanos < 0) {
      seconds -= 1;
      nanos += 1e9;
    }
    return [seconds, nanos];
  };
  hrtime.bigint = () => BigInt(now) * 1000000n;
  put(process, 'hrtime', hrtime);
  // Time belongs to the current turn (turn.js). A timer an earlier turn set (the app's start, or a request already answered) never
  // fires, so background work waits instead of running into the request being answered. An interval that misses its turn is cleared.
  const wait = (schedule, clear) => function ghostTimer(callback, ...rest) {
    if (typeof callback !== 'function') return schedule(callback, ...rest);
    const mine = owner();
    const handle = schedule(function ghostTick(...args) {
      if (!isCurrent(mine)) {
        clear(handle);
        return undefined;
      }
      return callback.apply(this, args);
    }, ...rest);
    return handle;
  };
  const later = (delay) => function ghostLater(...args) {
    const mine = owner();
    return delay(...args).then((value) => (isCurrent(mine) ? value : new Promise(() => {})));
  };
  const timed = {
    setTimeout: wait(timers.setTimeout, timers.clearTimeout),
    setInterval: wait(timers.setInterval, timers.clearInterval),
    setImmediate: wait(timers.setImmediate, timers.clearImmediate),
  };
  const promised = { setTimeout: later(timersPromises.setTimeout), setImmediate: later(timersPromises.setImmediate) };
  timed.setTimeout[promisify.custom] = promised.setTimeout;
  timed.setImmediate[promisify.custom] = promised.setImmediate;
  for (const [name, value] of Object.entries(timed)) {
    put(globalThis, name, value);
    put(timers, name, value);
  }
  for (const [name, value] of Object.entries(promised)) put(timersPromises, name, value);
  syncBuiltinESMExports();
}
