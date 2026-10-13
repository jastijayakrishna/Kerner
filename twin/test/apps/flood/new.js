import { MessageChannel } from 'node:worker_threads';
import { send } from '../http.js';

// On /flood it fails without end, in work no turn holds (a message passed back and forth between two ports), so its ghost keeps
// reporting itself broken until it is stopped. Every other request is answered as the old version answers it.
export function createApp() {
  return (req, res) => {
    if (req.url !== '/flood') return send(res, 200, { ok: true });
    const { port1, port2 } = new MessageChannel();
    port2.on('message', () => {
      for (let i = 0; i < 50; i++) Promise.reject(new Error('flood'));
      port1.postMessage(0);
    });
    port1.postMessage(0);
    return undefined;
  };
}
