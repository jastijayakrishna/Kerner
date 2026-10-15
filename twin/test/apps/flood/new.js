import { MessageChannel } from 'node:worker_threads';
import { send } from '../http.js';



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
