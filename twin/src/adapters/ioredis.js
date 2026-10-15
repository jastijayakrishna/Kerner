import { redisIsWrite, redisKey } from '../keys.js';
import { revive } from '../serialize.js';
import { mine } from '../turn.js';
import { CALL_BYTES, spend } from '../capture.js';

const PATCHED = Symbol.for('kern.twin.ioredis');
const WATCHED = Symbol.for('kern.twin.ioredis.command');

const redisOf = (module) => module.default ?? module.Redis ?? module;

function watch(sampled, command, installedLimit) {
  const maxBodyBytes = sampled.maxBodyBytes ?? installedLimit;
  const args = [...(command.args ?? [])];
  const call = { kind: 'redis', key: redisKey(command.name, args), write: redisIsWrite(command.name), command: String(command.name), args, reply: null, error: null };
  command[WATCHED] = true;
  sampled.io.push(call);
  spend(sampled, CALL_BYTES);
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
      const size = Buffer.byteLength(JSON.stringify(value) ?? '');
      if (size > maxBodyBytes) sampled.problem ??= 'a Redis reply over the limit';
      else spend(sampled, size);
    } catch {
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
    if (sampled && !sampled.done && typeof command?.name === 'string' && !command[WATCHED]) {
      try {
        watch(sampled, command, maxBodyBytes);
      } catch {
        sampled.problem ??= 'a Redis command Kern could not name';
      }
    }
    return send.call(this, command, stream);
  };
  proto[PATCHED] = true;
}

function replay({ appRequire, take, wrote, note }) {
  const proto = redisOf(appRequire('ioredis')).prototype;
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
