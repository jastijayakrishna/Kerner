import { send } from '../http.js';

// Answers whether the module preloaded into the app's process (preload.cjs) also ran here.
export function createApp() {
  return (req, res) => send(res, 200, { preloaded: globalThis.__kernPreloaded === true });
}
