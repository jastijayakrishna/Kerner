import { isMainThread } from 'node:worker_threads';
import { createApp as refactored } from '../refactor/new.js';



export function createApp() {
  if (isMainThread) {
    globalThis.kernTwinMainThreadLoads = (globalThis.kernTwinMainThreadLoads ?? 0) + 1;
    throw new Error("this version cannot start in the app's thread");
  }
  return refactored();
}
