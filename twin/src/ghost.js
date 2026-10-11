import { Worker } from 'node:worker_threads';

const THREAD = new URL('./worker.js', import.meta.url);

/** The app's environment for a ghost, without NODE_OPTIONS: a module preloaded there would run in the ghost before its seal (measured). */
export function ghostEnv() {
  const env = { ...process.env };
  delete env.NODE_OPTIONS;
  return env;
}

/** Sends one request to one ghost and answers its result, or why there is none. `dirty` means the ghost must be restarted. */
function ask(worker, id, job, timeoutMs) {
  return new Promise((resolve) => {
    let dirty = false;
    let timer = null;
    const done = (result) => {
      clearTimeout(timer);
      worker.off('message', onMessage);
      worker.off('exit', onExit);
      resolve({ ...result, dirty: dirty || Boolean(result.timedOut || result.exited) });
    };
    const onMessage = (message) => {
      if (message?.type === 'dirty') dirty = true;
      else if (message?.type === 'result' && message.id === id) done(message.result);
    };
    const onExit = (code) => done({ error: `the ghost stopped (exit ${code})`, exited: true });
    timer = setTimeout(() => done({ error: `no answer within ${timeoutMs} ms`, timedOut: true }), timeoutMs);
    worker.on('message', onMessage);
    worker.on('exit', onExit);
    worker.postMessage({ type: 'job', id, job });
  });
}

/**
 * Starts one ghost: one version of the app in its own thread, sealed, waiting for requests. Resolves once that version has loaded,
 * with { kind: 'thread', ask(id, job, timeoutMs), terminate(), onDirty(listener) }.
 * `side` is { module, export }; `settings` is { startMs, memoryMb, ghostExecArgv, addons }.
 */
export function startGhost(side, settings) {
  return new Promise((resolve, reject) => {
    // The ghost drops its own console output (worker.js). Reading it here instead would keep the process alive.
    // No Node flags and no NODE_OPTIONS from the app: a module they preload would run in the ghost before the seal goes on (measured:
    // execArgv [] alone still runs a NODE_OPTIONS preload). Flags a ghost needs, a TypeScript loader say, are given as ghostExecArgv.
    const worker = new Worker(THREAD, {
      workerData: { module: side.module, export: side.export, addons: settings.addons ?? [] },
      execArgv: settings.ghostExecArgv ?? [],
      env: ghostEnv(),
      resourceLimits: { maxOldGenerationSizeMb: settings.memoryMb },
    });
    let timer = null;
    const cleanup = () => {
      clearTimeout(timer);
      worker.off('message', onMessage);
      worker.off('error', onError);
      worker.off('exit', onExit);
    };
    const fail = (error) => {
      cleanup();
      worker.terminate();
      reject(error);
    };
    const onMessage = (message) => {
      if (message?.type !== 'ready') return;
      cleanup();
      const dirty = [];
      // From here on an error (running out of memory, say) ends only this ghost. Without a listener, Node would throw the worker's
      // 'error' event in the thread that started it: one must always be attached.
      worker.on('error', () => dirty.forEach((listener) => listener()));
      worker.on('message', (m) => {
        if (m?.type === 'dirty') dirty.forEach((listener) => listener());
      });
      // A ghost never keeps a process alive; while it answers, ask()'s time limit does. This comes after the 'message' listener above:
      // Node refs a worker again whenever its first 'message' listener is added.
      worker.unref();
      resolve({
        kind: 'thread',
        ask: (id, job, timeoutMs) => ask(worker, id, job, timeoutMs),
        terminate: () => worker.terminate(),
        onDirty: (listener) => dirty.push(listener),
      });
    };
    const onError = (error) => fail(error);
    const onExit = (code) => fail(new Error(`stopped while loading (exit ${code})`));
    timer = setTimeout(() => fail(new Error(`did not load within ${settings.startMs} ms`)), settings.startMs);
    worker.on('message', onMessage);
    worker.on('error', onError);
    worker.on('exit', onExit);
  });
}
