export function ghostEnv() {
  const env = { ...process.env };
  delete env.NODE_OPTIONS;
  return env;
}

export const stopped = (ghost) => ghost.threadId === -1 || (ghost.exitCode ?? null) !== null || (ghost.signalCode ?? null) !== null;

export function ask(ghost, post, id, job, timeoutMs) {
  if (stopped(ghost)) return Promise.resolve({ error: 'the ghost had stopped before this request', exited: true, gone: true, dirty: true });
  return new Promise((resolve) => {
    let dirty = false;
    let timer = null;
    const done = (result) => {
      clearTimeout(timer);
      ghost.off('message', onMessage);
      ghost.off('exit', onExit);
      resolve({ ...result, dirty: dirty || Boolean(result.timedOut || result.exited) });
    };
    const onMessage = (message) => {
      if (message?.type === 'dirty') dirty = true;
      else if (message?.type === 'result' && message.id === id) done(message.result);
    };
    const onExit = (code) => done({ error: `the ghost stopped (exit ${code})`, exited: true });
    timer = setTimeout(() => done({ error: `no answer within ${timeoutMs} ms`, timedOut: true }), timeoutMs);
    ghost.on('message', onMessage);
    ghost.on('exit', onExit);
    post({ type: 'job', id, job });
  });
}

export function answer(post, message, settings) {
  const reply = settings.onRealRead ? Promise.resolve().then(() => settings.onRealRead(message)) : Promise.resolve({ error: 'no real reads here' });
  reply.then(
    (value) => post({ type: 'answer', id: message.id, answer: value }),
    (error) => post({ type: 'answer', id: message.id, answer: { error: String(error?.message ?? error) } }),
  );
}
