import { runGhost } from './ghost-core.js';

process.on('disconnect', () => process.exit(0));
const side = await new Promise((resolve) => {
  process.once('message', (message) => resolve(message?.side));
});
try {
  await runGhost(side, {
    send: (message) => process.send(message),
    onMessage: (listener) => process.on('message', listener),
    beforeSeal: null,
  });
} catch (error) {
  process.send({ type: 'failed', reason: String(error?.message ?? error).slice(0, 500) }, () => process.exit(1));
}
