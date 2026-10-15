import { pbkdf2 } from 'node:crypto';
import { send } from '../http.js';



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
