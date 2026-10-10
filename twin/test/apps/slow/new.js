import { createApp as createOld } from '../refactor/old.js';

// The same answers, 300 ms later: slow enough that a burst of requests fills the twin's queue.
export function createApp() {
  const old = createOld();
  return (req, res) => {
    setTimeout(() => old(req, res), 300);
  };
}
