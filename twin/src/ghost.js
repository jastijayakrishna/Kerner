import { Worker } from 'node:worker_threads';
import { answer, ask, ghostEnv, stopped } from './ghost-common.js';
import { startProcessGhost } from './process-ghost.js';

export { ghostEnv };

const THREAD = new URL('./worker.js', import.meta.url);

/**
 * Starts one ghost (one version of the app, sealed, waiting for requests) and resolves once that version has loaded. It answers
 * { kind, loadLines, ask(id, job, timeoutMs), terminate(), onDirty(listener), stopped() }.
 * `settings.kind` picks a thread ('thread', the default) or a process of its own ('process'). The other settings are
 * { startMs, memoryMb, ghostExecArgv, addons, coverage, adapters, overlay, realReads, sandbox, onRealRead, maxBodyBytes }.
 */
export function startGhost(side, settings) {
  if (settings.kind === 'process') return startProcessGhost(side, settings);
  return new Promise((resolve, reject) => {
    // The ghost drops its own console output (worker.js); reading it here instead would keep the process alive. No Node flags and no
    // NODE_OPTIONS from the app: a module they preload would run in the ghost before the seal goes on. Flags a ghost needs are given
    // as ghostExecArgv.
    const worker = new Worker(THREAD, {
      workerData: {
        module: side.module, export: side.export, addons: settings.addons ?? [], coverage: settings.coverage ?? null,
        adapters: settings.adapters ?? [], overlay: Boolean(settings.overlay), realReads: settings.realReads ?? [],
        load: settings.load ?? null, pgReads: Boolean(settings.pgReads), maxBodyBytes: settings.maxBodyBytes ?? null,
      },
      execArgv: settings.ghostExecArgv ?? [],
      env: ghostEnv(),
      resourceLimits: { maxOldGenerationSizeMb: settings.memoryMb },
    });
    let timer = null;
    let loadLines = [];
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
      if (message?.type === 'load-lines') loadLines = message.lines;
      if (message?.type !== 'ready') return;
      cleanup();
      const dirty = [];
      // From here on an error (running out of memory, say) ends only this ghost. Without a listener, Node would throw the worker's
      // 'error' event in the thread that started it: one must always be attached.
      worker.on('error', () => dirty.forEach((listener) => listener()));
      worker.on('message', (m) => {
        if (m?.type === 'dirty') dirty.forEach((listener) => listener());
        else if (m?.type === 'real-read') answer((reply) => worker.postMessage(reply), m, settings);
      });
      // A ghost never keeps a process alive; while it answers, ask()'s time limit does. This comes after the 'message' listener above:
      // Node refs a worker again whenever its first 'message' listener is added.
      worker.unref();
      resolve({
        kind: 'thread',
        loadLines,
        ask: (id, job, timeoutMs) => ask(worker, (m) => worker.postMessage(m), id, job, timeoutMs),
        terminate: () => worker.terminate(),
        onDirty: (listener) => dirty.push(listener),
        stopped: () => stopped(worker),
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
