import { send } from '../http.js';


export function createApp() {
  return (req, res) => setTimeout(() => send(res, 200, { interval: false, wait: false, yield: false }), 50);
}
