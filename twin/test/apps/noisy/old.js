import fs from 'node:fs';
import { send } from '../http.js';


export function createApp() {
  console.log('ghost output at load');
  return (req, res) => {
    console.log('ghost output: console.log');
    console.error('ghost output: console.error');
    process.emitWarning('ghost output: a warning');
    fs.writeSync(1, 'ghost output: fd 1\n');
    fs.write(2, 'ghost output: fd 2\n', () => {});
    process.stdout.write('ghost output: process.stdout\n', () => send(res, 200, { ok: true }));
  };
}
