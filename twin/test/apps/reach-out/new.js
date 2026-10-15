import { execFileSync } from 'node:child_process';
import { send } from '../http.js';



export function createApp() {
  return (req, res) => {
    let formats = ['png'];
    try {
      execFileSync(process.execPath, ['-e', '']);
      formats = ['png', 'webp'];
    } catch {

    }
    send(res, 200, { formats });
  };
}
