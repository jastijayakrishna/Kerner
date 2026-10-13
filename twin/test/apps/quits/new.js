import { pbkdf2 } from 'node:crypto';
import { createApp as refactored } from '../refactor/new.js';

// Answers as the old version does, then ends its own process once some slow background work is done: a shutdown meant for a signal,
// run after every request. The work (a hash on Node's thread pool) outlives the request's turn, so in a ghost the process ends between
// two requests, after the answer has gone out.
export function createApp() {
  const serve = refactored();
  return (req, res) => {
    serve(req, res);
    pbkdf2('shutdown', 'salt', 100000, 32, 'sha256', () => process.exit(0));
  };
}
