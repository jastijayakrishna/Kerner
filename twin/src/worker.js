import { parentPort, workerData } from 'node:worker_threads';
import fs from 'node:fs';
import { devNull } from 'node:os';
import { syncBuiltinESMExports } from 'node:module';
import { runGhost } from './ghost-core.js';

// A thread ghost: the ghost core, with its messages carried by the worker's port. A thread shares file descriptors 1 and 2 with the app,
// so its output is dropped here, before the seal goes on (the seal would refuse opening the null device).
await runGhost(workerData, {
  send: (message) => parentPort.postMessage(message),
  onMessage: (listener) => parentPort.on('message', listener),
  beforeSeal: muteOutput,
});

/** A ghost's output is not the app's: it is dropped here, never mixed into production logs. */
function muteOutput() {
  // The console and process.stdout: callers waiting for a write to finish are still told it did.
  for (const stream of [process.stdout, process.stderr]) {
    stream.write = (chunk, encoding, callback) => {
      const done = typeof encoding === 'function' ? encoding : callback;
      if (typeof done === 'function') process.nextTick(done);
      return true;
    };
  }
  // Loggers such as pino write straight to file descriptors 1 and 2, past the console; a worker shares them with the app. In a ghost
  // those writes go to the null device, so each call still gives its usual result.
  const sink = fs.openSync(devNull, 'w');
  for (const name of ['write', 'writeSync', 'writev', 'writevSync']) {
    const real = fs[name];
    fs[name] = function ghostWrite(fd, ...rest) {
      return real.call(this, fd === 1 || fd === 2 ? sink : fd, ...rest);
    };
  }
  syncBuiltinESMExports();
}
