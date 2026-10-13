import { execFileSync } from 'node:child_process';
import { send } from '../http.js';

// Asks a program which formats it knows, and falls back to the old answer when it cannot: on the same request, the new code reaches
// for something the old code never did. (The program is Node itself, doing nothing: harmless if it ever ran.)
export function createApp() {
  return (req, res) => {
    let formats = ['png'];
    try {
      execFileSync(process.execPath, ['-e', '']);
      formats = ['png', 'webp'];
    } catch {
      // No program here: the old answer.
    }
    send(res, 200, { formats });
  };
}
