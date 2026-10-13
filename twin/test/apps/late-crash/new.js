import { pbkdf2 } from 'node:crypto';
import { send } from '../http.js';

// The audit now fails after the answer has gone out: an uncaught error in work the request left running. It also leaves `open` counting
// an entry that will never be finished, which no fresh copy of the app has.
export function createApp() {
  let open = 0;
  return (req, res) => {
    send(res, 200, { open });
    open++;
    pbkdf2('audit', 'salt', 100000, 32, 'sha256', () => {
      throw new Error('the audit entry could not be written');
    });
  };
}
