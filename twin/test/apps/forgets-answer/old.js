import { send } from '../http.js';


export function createApp() {
  return (req, res) => send(res, 200, { report: 'ready', url: req.url });
}
