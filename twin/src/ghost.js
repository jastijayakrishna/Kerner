import { Worker } from 'node:worker_threads';
import { answer, ask, ghostEnv, stopped } from './ghost-common.js';
import { startProcessGhost } from './process-ghost.js';

export { ghostEnv };

const THREAD = new URL('./worker.js', import.meta.url);

export function startGhost(side, settings) {
  if (settings.kind === 'process') return startProcessGhost(side, settings);
  return new Promise((resolve, reject) => {
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
      worker.on('error', (error) => dirty.forEach((listener) => listener({ type: 'dirty', error: `the ghost failed: ${error?.message ?? error}` })));
      worker.on('exit', (code) => dirty.forEach((listener) => listener({ type: 'dirty', error: `the ghost stopped (exit ${code})` })));
      worker.on('message', (m) => {
        if (m?.type === 'dirty') dirty.forEach((listener) => listener(m));
        else if (m?.type === 'real-read') answer((reply) => worker.postMessage(reply), m, settings);
      });
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
