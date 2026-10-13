import { pgIsWrite, pgKey } from '../keys.js';
import { revive } from '../serialize.js';
import { mine, never } from '../turn.js';
import { CALL_BYTES, spend } from '../capture.js';

// node-postgres (pg).
// - In the app's thread, every query of a sampled request is written down at Client.prototype.query, which Pool.query goes through
//   too: its text, values and result, or its error.
// - In a ghost, no client ever connects: each query is answered from the recording.
// Cursors and query streams (Submittables) are not copied: a request that uses one is skipped.
const PATCHED = Symbol.for('kern.twin.pg');
const ERROR_FIELDS = ['code', 'detail', 'hint', 'position', 'schema', 'table', 'column', 'dataType', 'constraint', 'severity', 'routine'];

/** A query's result as it is kept and sent between threads: command, row count, rows, and the fields' names and types. */
export function resultOf(result) {
  if (Array.isArray(result)) return result.map(resultOf);
  return { command: result?.command ?? null, rowCount: result?.rowCount ?? null, rows: result?.rows ?? [], fields: (result?.fields ?? []).map((f) => ({ name: f.name, dataTypeID: f.dataTypeID })) };
}

function errorOf(error) {
  const out = { message: String(error?.message ?? error) };
  for (const field of ERROR_FIELDS) if (error?.[field] !== undefined) out[field] = error[field];
  return out;
}

const sizeOf = (value) => {
  try {
    return Buffer.byteLength(JSON.stringify(value) ?? '');
  } catch {
    return Infinity;
  }
};

function record({ appRequire, current, maxBodyBytes }) {
  const pg = appRequire('pg');
  const proto = pg.Client.prototype;
  if (proto[PATCHED]) return;
  const query = proto.query;
  proto.query = function kernQuery(config, values, callback) {
    const sampled = current.getStore();
    if (!sampled || sampled.done || config == null) return query.call(this, config, values, callback);
    if (typeof config.submit === 'function') {
      sampled.problem ??= 'a Postgres cursor or query stream, which Kern does not copy';
      return query.call(this, config, values, callback);
    }
    const given = typeof values === 'function' ? undefined : values;
    const done = typeof values === 'function' ? values : callback;
    // The limit of the twin whose request this is: pg is patched once per process, by the first twin.
    const limit = sampled.maxBodyBytes ?? maxBodyBytes;
    let call;
    try {
      const text = typeof config === 'string' ? config : String(config.text ?? '');
      const params = given ?? config.values ?? [];
      call = { kind: 'pg', key: pgKey(text, params), write: pgIsWrite(text), text, values: params, result: null, error: null };
    } catch {
      // Values JSON cannot write (a circle, a depth past its limit) cannot be named. The call is not copied, and goes to pg untouched:
      // the app gets pg's own answer, never a throw from the twin.
      sampled.problem ??= 'a Postgres call Kern could not name';
      return query.call(this, config, values, callback);
    }
    sampled.io.push(call);
    spend(sampled, CALL_BYTES);
    let settle;
    const tracked = new Promise((resolve) => {
      settle = resolve;
    });
    sampled.pending.add(tracked);
    tracked.then(() => sampled.pending.delete(tracked));
    const heard = (error, result) => {
      try {
        if (error) call.error = errorOf(error);
        else {
          call.result = resultOf(result);
          const size = sizeOf(call.result);
          if (size > limit) sampled.problem ??= 'a Postgres result over the limit';
          else spend(sampled, size);
        }
      } catch {
        sampled.problem ??= 'a Postgres result Kern could not copy';
      }
      settle();
    };
    // The query goes to pg exactly as the app made it; only its callback is wrapped, so pg's own behaviour is unchanged.
    if (typeof config === 'object' && typeof config.callback === 'function') {
      const own = config.callback;
      return query.call(this, { ...config, callback: (error, result) => { heard(error, result); own(error, result); } }, given);
    }
    if (typeof done === 'function') return query.call(this, config, given, (error, result) => { heard(error, result); done(error, result); });
    return new this._Promise((resolve, reject) => {
      query.call(this, config, given, (error, result) => {
        heard(error, result);
        if (error) reject(error);
        else resolve(result);
      });
    });
  };
  proto[PATCHED] = true;
}

function resultFrom(result) {
  if (Array.isArray(result)) return result.map(resultFrom);
  return { command: result.command, rowCount: result.rowCount, oid: null, rows: revive(result.rows), fields: result.fields };
}

function errorFrom(pg, recorded) {
  const error = typeof pg.DatabaseError === 'function' ? new pg.DatabaseError(recorded.message, 0, 'error') : new Error(recorded.message);
  return Object.assign(error, recorded);
}

function replay({ appRequire, take, wrote, note, ask = null, pgReads = false }) {
  const pg = appRequire('pg');
  const proto = pg.Client.prototype;
  // A ghost's client never connects: it is connected at once, and every query is answered from the recording.
  proto.connect = function ghostConnect(callback) {
    if (typeof callback === 'function') {
      process.nextTick(callback, null);
      return undefined;
    }
    return Promise.resolve(this);
  };
  proto.end = function ghostEnd(callback) {
    if (typeof callback === 'function') {
      process.nextTick(callback);
      return undefined;
    }
    return Promise.resolve();
  };
  proto.query = function ghostQuery(config, values, callback) {
    if (config == null) throw new TypeError('Client was passed a null or undefined query');
    // A query the current turn did not make waits forever (turn.js): no answer, no callback, nothing written down.
    if (!mine()) {
      if (typeof config.submit === 'function') return config;
      return typeof values === 'function' || typeof callback === 'function' || typeof config.callback === 'function' ? undefined : never();
    }
    if (typeof config.submit === 'function') {
      note('denied', 'a Postgres cursor or query stream');
      process.nextTick(() => config.handleError?.(new Error('Kern ghosts do not run Postgres cursors or streams')));
      return config;
    }
    const given = typeof values === 'function' ? undefined : values;
    const own = typeof config === 'object' && typeof config.callback === 'function' ? config.callback : null;
    const done = typeof values === 'function' ? values : own ?? callback;
    const text = typeof config === 'string' ? config : String(config.text ?? '');
    const key = pgKey(text, given ?? config.values ?? []);
    const write = pgIsWrite(text);
    if (write) wrote(key);
    const recorded = take(key);
    const shown = `PG ${text.replace(/\s+/g, ' ').trim().slice(0, 120)}`;
    // With pgReads, a read the recording cannot answer (one the new code makes and the old never did, say) is made for real by the
    // twin's thread: read-only, once for both ghosts of the request. A write never is.
    if (!recorded && !write && pgReads && ask) {
      const real = ask({ type: 'real-read', kind: 'pg', text, values: given ?? config.values ?? [] }).then((answer) => {
        if (answer?.error) {
          note('unmatched-read', shown);
          return { error: new Error('Connection terminated unexpectedly') };
        }
        note('real-read', shown);
        return { result: resultFrom(answer.result) };
      });
      if (typeof done === 'function') {
        real.then(({ error, result }) => done(error ?? null, result));
        return undefined;
      }
      return real.then(({ error, result }) => (error ? Promise.reject(error) : result));
    }
    if (!recorded) note(write ? 'unmatched-write' : 'unmatched-read', shown);
    const error = !recorded ? new Error('Connection terminated unexpectedly') : recorded.error ? errorFrom(pg, recorded.error) : null;
    const result = error ? undefined : resultFrom(recorded.result);
    if (typeof done === 'function') {
      process.nextTick(done, error, result);
      return undefined;
    }
    return error ? Promise.reject(error) : Promise.resolve(result);
  };
}

export default { kind: 'pg', record, replay };
