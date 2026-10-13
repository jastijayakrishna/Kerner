import { send } from '../http.js';

/** Every customer as one table: over 2 MiB of JSON. */
export const table = () => ({ rows: Array.from({ length: 70000 }, (_, id) => ({ id, name: `customer ${id}` })) });

// Answers with the whole customer table where the old version answered a count: a body larger than the twin keeps.
export function createApp() {
  return (req, res) => send(res, 200, table());
}
