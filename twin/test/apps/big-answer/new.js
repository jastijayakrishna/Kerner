import { send } from '../http.js';


export const table = () => ({ rows: Array.from({ length: 70000 }, (_, id) => ({ id, name: `customer ${id}` })) });


export function createApp() {
  return (req, res) => send(res, 200, table());
}
