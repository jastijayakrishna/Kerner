import { createRequire } from 'node:module';

const pino = createRequire(process.env.KERN_PROOF_PINO)('pino');
pino.transport = () => pino.destination({ dest: 1, sync: true });
