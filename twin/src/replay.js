import { startGhost } from './ghost.js';
import { toRun } from './coordinator.js';

export async function replayCase(value, version, settings = {}) {
  const ghost = await startGhost(version, {
    startMs: settings.startMs ?? 20000, memoryMb: settings.memoryMb ?? 512, ghostExecArgv: settings.ghostExecArgv ?? [], addons: settings.addons ?? [],
    adapters: value.adapters ?? ['fetch'], overlay: Boolean(value.overlay), realReads: [],
  });
  try {
    const job = { start: value.world.start, seed: value.world.seed, request: value.request, io: value.io, coverage: false };
    return toRun(await ghost.ask(1, job, settings.timeoutMs ?? 5000));
  } finally {
    await ghost.terminate();
  }
}
