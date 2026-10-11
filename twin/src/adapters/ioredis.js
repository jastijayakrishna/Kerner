import { redisIsWrite, redisKey } from '../keys.js';
import { revive } from '../serialize.js';
import { mine } from '../turn.js';

// ioredis.
// - In the app's thread, every command of a sampled request is written down at Redis.prototype.sendCommand (pipelines and transactions
//   send theirs through it too), with the raw reply as the server sent it, before ioredis transforms it.
// - In a ghost, no client connects: it is ready at once, and each command is answered from the recording. Its raw reply goes through
//   ioredis's own transformation once, as a real reply would.
// Redis Cluster and subscriber mode are not covered.
const PATCHED = Symbol.for('kern.twin.ioredis');
const WATCHED = Symbol.for('kern.twin.ioredis.command');

const redisOf = (module) => module.default ?? module.Redis ?? module;

/** Writes down one command and its raw reply. A command sent again (from the offline queue) is written down once. */
function watch(sampled, command, maxBodyBytes) {
  command[WATCHED] = true;
  const args = [...(command.args ?? [])];
  const call = { kind: 'redis', key: redisKey(command.name, args), write: redisIsWrite(command.name), command: String(command.name), args, reply: null, error: null };
  sampled.io.push(call);
  let settle;
  const tracked = new Promise((resolve) => {
    settle = resolve;
  });
  sampled.pending.add(tracked);
  tracked.then(() => sampled.pending.delete(tracked));
  const { resolve, reject } = command;
  command.resolve = (value) => {
    call.reply = value;
    try {
      if (Buffer.byteLength(JSON.stringify(value) ?? '') > maxBodyBytes) sampled.problem ??= 'a Redis reply over the limit';
    } catch {
      // A reply JSON cannot show is still copied between threads as it is.
    }
    settle();
    return resolve(value);
  };
  command.reject = (error) => {
    call.error = { message: String(error?.message ?? error), name: error?.name ?? 'Error' };
    settle();
    return reject(error);
  };
}

function record({ appRequire, current, maxBodyBytes }) {
  const proto = redisOf(appRequire('ioredis')).prototype;
  if (proto[PATCHED]) return;
  const send = proto.sendCommand;
  proto.sendCommand = function kernSendCommand(command, stream) {
    const sampled = current.getStore();
    if (sampled && !sampled.done && typeof command?.name === 'string' && !command[WATCHED]) watch(sampled, command, maxBodyBytes);
    return send.call(this, command, stream);
  };
  proto[PATCHED] = true;
}

function replay({ appRequire, take, wrote, note }) {
  const proto = redisOf(appRequire('ioredis')).prototype;
  // A ghost's client never connects: it is ready at once.
  proto.connect = function ghostConnect(callback) {
    if (this.status !== 'ready') {
      this.setStatus('connect');
      this.setStatus('ready');
    }
    const ready = Promise.resolve();
    if (typeof callback === 'function') ready.then(() => callback(null));
    return ready;
  };
  proto.disconnect = function ghostDisconnect() {
    this.setStatus('end');
  };
  proto.sendCommand = function ghostSendCommand(command) {
    // A command the current turn did not send waits forever (turn.js): its promise never settles.
    if (!mine()) return command.promise;
    command.setReplyContext?.(this.condition ?? this.options);
    const key = redisKey(command.name, command.args ?? []);
    const write = redisIsWrite(command.name);
    if (write) wrote(key);
    const recorded = take(key);
    if (!recorded) {
      note(write ? 'unmatched-write' : 'unmatched-read', `REDIS ${String(command.name).toUpperCase()}`);
      command.reject(new Error('Connection is closed.'));
    } else if (recorded.error) command.reject(Object.assign(new Error(recorded.error.message), { name: recorded.error.name }));
    else command.resolve(revive(recorded.reply));
    return command.promise;
  };
}

export default { kind: 'ioredis', record, replay };
