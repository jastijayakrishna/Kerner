// What thread ghosts and process ghosts share: the environment they start with, how a request is asked of one, and how its questions
// are answered.

/** The app's environment for a ghost, without NODE_OPTIONS: a module preloaded there would run in the ghost before its seal (measured). */
export function ghostEnv() {
  const env = { ...process.env };
  delete env.NODE_OPTIONS;
  return env;
}

/** Whether a ghost has stopped: a Worker's threadId is -1 once its thread has ended, and a ChildProcess has its exit code or signal. */
const stopped = (ghost) => ghost.threadId === -1 || (ghost.exitCode ?? null) !== null || (ghost.signalCode ?? null) !== null;

/**
 * Sends one request to one ghost and answers its result, or why there is none. `ghost` is the Worker or the ChildProcess (both emit
 * 'message' and 'exit'); `post` sends it a message. `dirty` means the ghost must be restarted.
 */
export function ask(ghost, post, id, job, timeoutMs) {
  // A ghost that stopped after its last answer (work a request left running ended its process, or ran it out of memory) emits no 'exit'
  // again, and a job posted to it goes nowhere. It answers at once that it had stopped (`gone`: it never saw this request), never after
  // the whole time limit as a hang.
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

/** Answers a ghost's question about a real read through the twin's own thread (settings.onRealRead); a failure is an answer too. */
export function answer(post, message, settings) {
  const reply = settings.onRealRead ? Promise.resolve().then(() => settings.onRealRead(message)) : Promise.resolve({ error: 'no real reads here' });
  reply.then(
    (value) => post({ type: 'answer', id: message.id, answer: value }),
    (error) => post({ type: 'answer', id: message.id, answer: { error: String(error?.message ?? error) } }),
  );
}
