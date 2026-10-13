import { pbkdf2 } from 'node:crypto';
import { send } from '../http.js';

// Answers at once, then writes an audit entry in the background: a slow hash on Node's thread pool, which outlives the request's turn.
// `open` counts the entries still being written.
export function createApp() {
  let open = 0;
  return (req, res) => {
    send(res, 200, { open });
    open++;
    pbkdf2('audit', 'salt', 100000, 32, 'sha256', () => {
      open--;
    });
  };
}
