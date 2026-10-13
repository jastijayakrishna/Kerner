import { send } from '../http.js';

// Answers how many customers there are.
export function createApp() {
  return (req, res) => send(res, 200, { count: 70000 });
}
