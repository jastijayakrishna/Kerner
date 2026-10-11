import { startGhost } from './ghost.js';
import { toRun } from './coordinator.js';

/**
 * Runs one version on a case: a fresh ghost thread, sealed, given the case's request in the case's world (clock and seed), with its
 * recording and the twin settings it was recorded under (adapters, overlay). Answers the run, as compareRuns reads one.
 * `version` is { module, export }; `settings` may set startMs, timeoutMs, memoryMb, ghostExecArgv and addons.
 */
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
