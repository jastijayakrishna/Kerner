import { createHash } from 'node:crypto';

// Every outgoing call is named `<shape> <payload>`, the same way in the real run and in both ghosts. The shape says what the call is:
// a method and an address, a query, a command and its key. The payload is a short hash of what it carried: a body, values, arguments.
// A ghost's clock is frozen at the request's arrival, so a payload that carries the time can never match exactly; its shape still does.

/** Methods that only read; every other method changes something somewhere. */
export const READS = new Set(['GET', 'HEAD', 'OPTIONS']);

export function digest(data) {
  return createHash('sha256').update(data).digest('hex');
}

const short = (data) => digest(data).slice(0, 16);

/** The shape of a call's name: all but its last word. */
export function shapeOf(key) {
  const text = String(key);
  const at = text.lastIndexOf(' ');
  return at < 0 ? text : text.slice(0, at);
}

/** Values as text that is the same in every thread: bytes as base64, dates as ISO text, big integers as digits. */
export function stable(value) {
  return JSON.stringify(value, function replace(key, plain) {
    const raw = this[key];
    if (raw instanceof Uint8Array) return { bytes: Buffer.from(raw.buffer, raw.byteOffset, raw.byteLength).toString('base64') };
    if (raw instanceof Date) return { date: Number.isNaN(raw.getTime()) ? 'invalid' : raw.toISOString() };
    if (typeof plain === 'bigint') return { bigint: plain.toString() };
    return plain;
  });
}

/** One HTTP call (fetch, or a node:http client): its shape is the method and the address, its payload the body. */
export function callKey(method, url, body) {
  const verb = String(method).toUpperCase();
  return `${verb} ${url} ${body && body.length ? short(body) : '-'}`;
}

export function isWrite(method) {
  return !READS.has(String(method).toUpperCase());
}

const SAVEPOINT = /^(savepoint|release(?:\s+savepoint)?|rollback\s+to(?:\s+savepoint)?)\s+("[^"]*"|[^\s;]+)\s*;?$/i;

/**
 * A query without its savepoint's name. The name is a label the client library numbers itself (knex: trx1, trx2, ... over the life of
 * the process), so a long-running app and a fresh ghost name the same savepoint differently: savepoints are matched in order instead.
 */
function savepointless(query) {
  const m = SAVEPOINT.exec(query);
  if (!m) return query;
  const verb = m[1].toLowerCase();
  return verb.startsWith('savepoint') ? 'SAVEPOINT ?' : verb.startsWith('release') ? 'RELEASE SAVEPOINT ?' : 'ROLLBACK TO SAVEPOINT ?';
}

/** One Postgres query: its shape is the query (spaces folded; the first 60 characters to read, a hash for all of it), its payload the values. */
export function pgKey(text, values) {
  const query = savepointless(String(text).replace(/\s+/g, ' ').trim());
  return `PG ${query.slice(0, 60)} ${short(query)} ${values?.length ? short(stable(values)) : '-'}`;
}

const PG_WRITES = /^(insert|update|delete|merge|create|alter|drop|truncate|grant|revoke|copy|call|do|lock|vacuum|refresh|comment|reindex|cluster|notify|security|import)\b/i;

/** Whether a query changes data: by its first word, a WITH whose statements change data, or a sequence moved in a SELECT. */
export function pgIsWrite(text) {
  const query = String(text).replace(/--[^\n]*|\/\*[\s\S]*?\*\//g, ' ').trim().replace(/^\(+\s*/, '');
  if (PG_WRITES.test(query)) return true;
  if (/^with\b/i.test(query)) return /\b(insert|update|delete|merge)\b/i.test(query);
  return /\b(nextval|setval)\s*\(/i.test(query);
}

const argText = (arg) => (arg instanceof Uint8Array ? Buffer.from(arg.buffer, arg.byteOffset, arg.byteLength).toString('base64') : String(arg));

/** One Redis command: its shape is the command and a hash of its first argument (usually the key), its payload all its arguments. */
export function redisKey(command, args) {
  const list = (args ?? []).map(argText);
  return `REDIS ${String(command).toUpperCase()} ${list.length ? short(list[0]) : '-'} ${list.length ? short(JSON.stringify(list)) : '-'}`;
}

const REDIS_READS = new Set(['GET', 'MGET', 'GETRANGE', 'STRLEN', 'EXISTS', 'TTL', 'PTTL', 'EXPIRETIME', 'TYPE', 'KEYS', 'SCAN', 'RANDOMKEY',
  'HGET', 'HMGET', 'HGETALL', 'HKEYS', 'HVALS', 'HLEN', 'HEXISTS', 'HSCAN', 'HSTRLEN', 'HRANDFIELD', 'LRANGE', 'LLEN', 'LINDEX', 'LPOS',
  'SMEMBERS', 'SISMEMBER', 'SMISMEMBER', 'SCARD', 'SSCAN', 'SRANDMEMBER', 'SINTER', 'SUNION', 'SDIFF', 'ZRANGE', 'ZRANGEBYSCORE', 'ZREVRANGE',
  'ZREVRANGEBYSCORE', 'ZRANGEBYLEX', 'ZREVRANGEBYLEX', 'ZSCORE', 'ZMSCORE', 'ZRANK', 'ZREVRANK', 'ZCARD', 'ZCOUNT', 'ZLEXCOUNT', 'ZSCAN',
  'BITCOUNT', 'BITPOS', 'GETBIT', 'PFCOUNT', 'XRANGE', 'XREVRANGE', 'XLEN', 'GEOPOS', 'GEODIST', 'GEOHASH', 'GEOSEARCH', 'DBSIZE', 'TIME',
  'PING', 'ECHO', 'INFO', 'SELECT', 'AUTH', 'HELLO', 'CLIENT', 'MULTI', 'WATCH', 'UNWATCH', 'DISCARD']);

/** Whether a Redis command changes data: every command but the known reads (and connection commands) does. */
export function redisIsWrite(command) {
  return !REDIS_READS.has(String(command).toUpperCase());
}

// Calls about the connection or the server, not the data: a version check, SET, SHOW or RESET, the current schema; Redis HELLO, AUTH,
// SELECT, CLIENT, INFO and PING. Their answer is the same however often they are asked, and how often they are asked depends on how many
// connections a pool happened to open, not on the code. (Keploy's "session" mocks: a short, explicit list, for the same reason.)
const PG_SESSION = /^PG (select version\(\)|select current_schema\(\)|show |set |reset |discard |deallocate )/i;
const REDIS_SESSION = /^REDIS (HELLO|AUTH|SELECT|CLIENT|INFO|PING) /;

/** Whether a call (by its key) is about the connection or the server: never used up when it is replayed. */
export const isSession = (key) => PG_SESSION.test(String(key)) || REDIS_SESSION.test(String(key));

/** Whether an allow-listed real read covers a call: 'GET https://rates.example/*' (a trailing * matches by prefix). GET and HEAD only. */
export function readAllowed(patterns, method, url) {
  const verb = String(method).toUpperCase();
  if (verb !== 'GET' && verb !== 'HEAD') return false;
  return (patterns ?? []).some((pattern) => {
    const [want, address] = String(pattern).split(' ', 2);
    if (want !== verb || !address) return false;
    return address.endsWith('*') ? url.startsWith(address.slice(0, -1)) : url === address;
  });
}
