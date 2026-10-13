import { pbkdf2 } from 'node:crypto';
import { createApp as refactored } from '../refactor/new.js';

// Answers as the old version does, then grows without end in background work that outlives the request's turn (a report built after a
// slow hash on Node's thread pool): the ghost's memory limit stops it between two requests, after its answer has gone out.
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
