import { send } from '../http.js';

// Answers after 300 ms: long enough for a client to give up first.
export function createApp() {
  return (req, res) => setTimeout(() => send(res, 200, { ok: true }), 300);
}
