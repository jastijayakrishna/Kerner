import { isMainThread } from 'node:worker_threads';
import { createApp as refactored } from '../refactor/new.js';

// A new version that loads in the ghosts but not in the app's own thread, where it would serve customers: say it needs a setting the
// ghosts have and the app's process lacks. Every try in the app's thread is counted.
export function createApp() {
  if (isMainThread) {
    globalThis.kernTwinMainThreadLoads = (globalThis.kernTwinMainThreadLoads ?? 0) + 1;
    throw new Error("this version cannot start in the app's thread");
  }
  return refactored();
}
