import { parentPort, workerData } from 'node:worker_threads';
import fs from 'node:fs';
import { devNull } from 'node:os';
import { syncBuiltinESMExports } from 'node:module';
import { runGhost } from './ghost-core.js';

await runGhost(workerData, {
  send: (message) => parentPort.postMessage(message),
  onMessage: (listener) => parentPort.on('message', listener),
  beforeSeal: muteOutput,
});

function muteOutput() {
  for (const stream of [process.stdout, process.stderr]) {
    stream.write = (chunk, encoding, callback) => {
      const done = typeof encoding === 'function' ? encoding : callback;
      if (typeof done === 'function') process.nextTick(done);
      return true;
    };
  }
  const sink = fs.openSync(devNull, 'w');
  for (const name of ['write', 'writeSync', 'writev', 'writevSync']) {
    const real = fs[name];
    fs[name] = function ghostWrite(fd, ...rest) {
      return real.call(this, fd === 1 || fd === 2 ? sink : fd, ...rest);
    };
  }
  syncBuiltinESMExports();
}
