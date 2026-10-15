import { createHash } from 'node:crypto';

export const READS = new Set(['GET', 'HEAD', 'OPTIONS']);

export function digest(data) {
  return createHash('sha256').update(data).digest('hex');
}

const short = (data) => digest(data).slice(0, 16);

export function shapeOf(key) {
  const text = String(key);
  const at = text.lastIndexOf(' ');
  return at < 0 ? text : text.slice(0, at);
}

export function stable(value) {
  return JSON.stringify(value, function replace(key, plain) {
    const raw = this[key];
    if (raw instanceof Uint8Array) return { bytes: Buffer.from(raw.buffer, raw.byteOffset, raw.byteLength).toString('base64') };
    if (raw instanceof Date) return { date: Number.isNaN(raw.getTime()) ? 'invalid' : raw.toISOString() };
    if (typeof plain === 'bigint') return { bigint: plain.toString() };
    return plain;
  });
}

export function jsonText(value, { sorted = false } = {}) {
  const parts = [];
  const frames = [];
  const resolved = (item, key) => (item !== null && typeof item === 'object' && typeof item.toJSON === 'function' ? item.toJSON(key) : item);
  const absent = (item) => item === undefined || typeof item === 'function' || typeof item === 'symbol';
  const open = (item) => {
    if (Array.isArray(item)) {
      parts.push('[');
      frames.push({ item, names: null, next: 0, written: 0 });
    } else if (item !== null && typeof item === 'object') {
      parts.push('{');
      frames.push({ item, names: sorted ? Object.keys(item).sort() : Object.keys(item), next: 0, written: 0 });
    } else parts.push(JSON.stringify(item));
  };
  const top = resolved(value, '');
  if (absent(top)) return undefined;
  open(top);
  while (frames.length) {
    const frame = frames.at(-1);
    const size = frame.names ? frame.names.length : frame.item.length;
    if (frame.next >= size) {
      parts.push(frame.names ? '}' : ']');
      frames.pop();
      continue;
    }
    const i = frame.next++;
    if (!frame.names) {
      if (i > 0) parts.push(',');
      const item = resolved(frame.item[i], String(i));
      if (absent(item)) parts.push('null');
      else open(item);
      continue;
    }
    const name = frame.names[i];
    const item = resolved(frame.item[name], name);
    if (absent(item)) continue;
    if (frame.written++ > 0) parts.push(',');
    parts.push(`${JSON.stringify(name)}:`);
    open(item);
  }
  return parts.join('');
}

const MOST_CANONICAL = 256 << 10;

function canonical(body) {
  if (typeof body !== 'string' && !(body instanceof Uint8Array)) return null;
  if (body.length > MOST_CANONICAL) return null;
  const text = typeof body === 'string' ? body : Buffer.from(body.buffer, body.byteOffset, body.byteLength).toString('utf8');
  const first = text.trimStart()[0];
  if (first !== '{' && first !== '[') return null;
  try {
    const value = JSON.parse(text);
    return value !== null && typeof value === 'object' ? jsonText(value, { sorted: true }) : null;
  } catch {
    return null;
  }
}

export function callKey(method, url, body) {
  const verb = String(method).toUpperCase();
  return `${verb} ${url} ${body && body.length ? short(canonical(body) ?? body) : '-'}`;
}

export function isWrite(method) {
  return !READS.has(String(method).toUpperCase());
}

const SAVEPOINT = /^(savepoint|release(?:\s+savepoint)?|rollback\s+to(?:\s+savepoint)?)\s+("[^"]*"|[^\s;]+)\s*;?$/i;

function savepointless(query) {
  const m = SAVEPOINT.exec(query);
  if (!m) return query;
  const verb = m[1].toLowerCase();
  return verb.startsWith('savepoint') ? 'SAVEPOINT ?' : verb.startsWith('release') ? 'RELEASE SAVEPOINT ?' : 'ROLLBACK TO SAVEPOINT ?';
}

export function pgKey(text, values) {
  const query = savepointless(String(text).replace(/\s+/g, ' ').trim());
  return `PG ${query.slice(0, 60)} ${short(query)} ${values?.length ? short(stable(values)) : '-'}`;
}

const PG_WRITES = /^(insert|update|delete|merge|create|alter|drop|truncate|grant|revoke|copy|call|do|lock|vacuum|refresh|comment|reindex|cluster|notify|security|import)\b/i;

export function pgIsWrite(text) {
  const query = String(text).replace(/--[^\n]*|\/\*[\s\S]*?\*\//g, ' ').trim().replace(/^\(+\s*/, '');
  if (PG_WRITES.test(query)) return true;
  if (/^with\b/i.test(query)) return /\b(insert|update|delete|merge)\b/i.test(query);
  return /\b(nextval|setval)\s*\(/i.test(query);
}

const argText = (arg) => (arg instanceof Uint8Array ? Buffer.from(arg.buffer, arg.byteOffset, arg.byteLength).toString('base64') : String(arg));

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

export function redisIsWrite(command) {
  return !REDIS_READS.has(String(command).toUpperCase());
}

const PG_SESSION = /^PG (select version\(\)|select current_schema\(\)|show |set |reset |discard |deallocate |begin\b|start transaction\b|commit\b|end\b|abort\b|rollback(?! to)\b)/i;
const REDIS_SESSION = /^REDIS (HELLO|AUTH|SELECT|CLIENT|INFO|PING) /;

export const isSession = (key) => PG_SESSION.test(String(key)) || REDIS_SESSION.test(String(key));

export function plainAddress(url) {
  try {
    const parsed = new URL(String(url));
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed : null;
  } catch {
    return null;
  }
}

export function readAllowed(patterns, method, url) {
  const verb = String(method).toUpperCase();
  if (verb !== 'GET' && verb !== 'HEAD') return false;
  const target = plainAddress(url);
  if (!target) return false;
  return (patterns ?? []).some((pattern) => {
    const [want, address] = String(pattern).split(' ', 2);
    if (want !== verb || !address) return false;
    const prefix = address.endsWith('*');
    const allowed = plainAddress(prefix ? address.slice(0, -1) : address);
    if (!allowed || allowed.origin !== target.origin) return false;
    return prefix ? target.href.startsWith(allowed.href) : target.href === allowed.href;
  });
}
