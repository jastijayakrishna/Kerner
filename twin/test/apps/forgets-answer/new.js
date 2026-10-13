import { send } from '../http.js';

// A new branch for weekly reports forgets to answer: the request hangs until the customer gives up.
export function createApp() {
  return (req, res) => {
    if (req.url === '/report?weekly=1') return;
    send(res, 200, { report: 'ready', url: req.url });
  };
}
