// Loaded with --import in each ghost, before its seal (the twin's ghostExecArgv). Infisical's logger always starts a pino transport,
// which is a thread of its own, and a ghost may start no thread. In a ghost, a transport becomes a direct write to standard output,
// which the ghost drops anyway. The app's own process keeps its real logger: this file runs only in ghosts.
import { createRequire } from 'node:module';

const pino = createRequire(process.env.KERN_PROOF_PINO)('pino');
pino.transport = () => pino.destination({ dest: 1, sync: true });
