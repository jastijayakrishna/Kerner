import { send } from '../http.js';


export function createApp() {
  return (req, res) => {
    if (req.url === '/report?weekly=1') return;
    send(res, 200, { report: 'ready', url: req.url });
  };
}
