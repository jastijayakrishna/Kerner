import net from 'node:net';
import { send } from '../http.js';

// Connects to the internet with the function a preload kept from before the seal.
export function createApp() {
  return (req, res) => {
    const socket = new net.Socket();
    socket.once('error', (error) => send(res, 200, { reached: error.code }));
    socket.once('connect', () => {
      socket.destroy();
      send(res, 200, { reached: 'connected' });
    });
    globalThis.__kernRealConnect.call(socket, 80, '1.1.1.1');
  };
}
