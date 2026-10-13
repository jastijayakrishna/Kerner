import { send } from '../http.js';

// No background work: after 50 ms, it answers that none ran.
export function createApp() {
  return (req, res) => setTimeout(() => send(res, 200, { interval: false, wait: false, yield: false }), 50);
}
