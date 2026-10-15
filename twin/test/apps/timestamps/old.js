import { randomUUID } from 'node:crypto';
import { send } from '../http.js';


export function createApp() {
  return (req, res) => send(res, 200, { at: new Date().toISOString(), id: randomUUID(), roll: Math.random() });
}
