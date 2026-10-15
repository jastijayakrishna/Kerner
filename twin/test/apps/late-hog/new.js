import { pbkdf2 } from 'node:crypto';
import { createApp as refactored } from '../refactor/new.js';



export function createApp() {
  const serve = refactored();
  return (req, res) => {
    serve(req, res);
    pbkdf2('report', 'salt', 100000, 32, 'sha256', () => {
      const hog = [];
      for (;;) hog.push(new Array(1e6).fill(1));
    });
  };
}
