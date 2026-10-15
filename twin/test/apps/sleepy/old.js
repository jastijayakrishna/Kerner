import { send } from '../http.js';


export function createApp() {
  return (req, res) => setTimeout(() => send(res, 200, { ok: true }), 300);
}
