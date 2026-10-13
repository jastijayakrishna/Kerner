import { send } from '../http.js';

// Answers which image formats it knows.
export function createApp() {
  return (req, res) => send(res, 200, { formats: ['png'] });
}
