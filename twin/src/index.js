import { wrapListener } from './capture.js';
import { recordFetch } from './record.js';
import { Coordinator } from './coordinator.js';

function version(value, name) {
  if (!value || typeof value.module !== 'string' || typeof value.export !== 'string') throw new TypeError(`${name} must be { module: '/absolute/path/to/app.js', export: 'createApp' }`);
  return { module: value.module, export: value.export };
}

function number(value, fallback, min, max, name) {
  const n = value ?? fallback;
  if (typeof n !== 'number' || !(n >= min && n <= max)) throw new TypeError(`${name} must be a number from ${min} to ${max}`);
  return n;
}

/**
 * Starts a twin: the old and the new version of the app run as ghosts and answer copies of sampled real requests.
 * The twin never changes what a customer gets; if anything goes wrong inside it, it switches itself off and says why.
 */
export async function createTwin(options = {}) {
  const settings = {
    old: version(options.old, 'old'),
    candidate: version(options.candidate, 'candidate'),
    sampleRate: number(options.sampleRate, 1, 0, 1, 'sampleRate'),
    timeoutMs: number(options.timeoutMs, 5000, 100, 60000, 'timeoutMs'),
    startMs: number(options.startMs, 20000, 1000, 120000, 'startMs'),
    queueLimit: number(options.queueLimit, 100, 1, 10000, 'queueLimit'),
    maxBodyBytes: number(options.maxBodyBytes, 1 << 20, 1024, 64 << 20, 'maxBodyBytes'),
    ghostMemoryMb: number(options.ghostMemoryMb, 512, 64, 16384, 'ghostMemoryMb'),
    maxRestarts: number(options.maxRestarts, 20, 0, 1000, 'maxRestarts'),
    log: options.log ?? null,
  };
  const coordinator = new Coordinator(settings);
  if (process.env.KERN_TWIN === 'off') await coordinator.disable('switched off with KERN_TWIN=off');
  else await coordinator.start();
  if (!coordinator.disabled) recordFetch(settings.maxBodyBytes);
  const inflight = new Set();
  const onError = (error) => {
    coordinator.disable(`recording failed: ${error?.message ?? error}`);
  };
  const deliver = (copy) => {
    inflight.add(copy);
    copy.then((capture) => coordinator.submit(capture), onError).finally(() => inflight.delete(copy));
  };
  const sample = () => Math.random() < settings.sampleRate && coordinator.admit();
  return {
    wrap: (listener) => wrapListener(listener, { sample, maxBodyBytes: settings.maxBodyBytes, deliver, onError }),
    stats: () => coordinator.snapshot(),
    differences: () => coordinator.differences(),
    idle: async () => {
      while (inflight.size) await Promise.allSettled([...inflight]);
      await coordinator.idle();
    },
    close: () => coordinator.close(),
  };
}
