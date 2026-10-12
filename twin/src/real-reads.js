import { createRequire } from 'node:module';
import { readCapped } from './record.js';
import { exactBytes } from './serialize.js';
import { pgIsWrite, readAllowed } from './keys.js';
import { resultOf } from './adapters/pg.js';

const PG_READ = /^\s*(select|with|table|values|show)\b/i;
// What acts even inside a read-only transaction: locks, sleeps, links to other servers, notifications, settings, other backends,
// files; sequences; row locks.
const PG_ACTS = /\b(pg_(try_)?advisory\w*|pg_sleep\w*|dblink\w*|lo_\w+|pg_notify|set_config|pg_terminate_backend|pg_cancel_backend|pg_reload_conf|pg_rotate_logfile|pg_file_write|pg_read_file|pg_read_binary_file|pg_ls_dir|nextval|setval|currval)\s*\(|\bfor\s+(update|no\s+key\s+update|share|key\s+share)\b/i;

// More than one statement: a semicolon with anything after it. pg sends a text without values whole, and Postgres runs every statement
// in it, so a COMMIT among them would end the read-only transaction and let the rest write. It is looked for in the text as written: a
// comment marker inside a string literal ('--', '/*') could otherwise hide what follows it.
const MORE_STATEMENTS = /;\s*\S/;

/** Whether a Postgres query may be made for real for the ghosts: one statement, a read by its first word, calling nothing that acts even when read-only. */
export function pgReadable(text) {
  if (MORE_STATEMENTS.test(String(text))) return false;
  const query = String(text).replace(/--[^\n]*|\/\*[\s\S]*?\*\//g, ' ');
  return PG_READ.test(query) && !pgIsWrite(query) && !PG_ACTS.test(query);
}

/**
 * The reader behind pgReads: the app's own pg package, a pool of at most two connections to `url` (a read replica or a read-only role,
 * ideally). Each query runs alone inside BEGIN TRANSACTION READ ONLY, so the database itself refuses any write, with a 5 s statement
 * timeout, and is rolled back. read(text, values) answers { result } or { error }.
 */
export function pgReader(url, appModule) {
  let pool = null;
  return {
    async read(text, values) {
      if (!pgReadable(text)) return { error: 'not a query Kern makes for real' };
      let client = null;
      try {
        if (!pool) {
          pool = new (createRequire(appModule)('pg').Pool)({ connectionString: url, max: 2 });
          // An idle connection that fails must not end the twin's thread.
          pool.on('error', () => {});
        }
        client = await pool.connect();
        await client.query('BEGIN TRANSACTION READ ONLY');
        await client.query("SET LOCAL statement_timeout = '5s'");
        return { result: resultOf(await client.query(text, values ?? [])) };
      } catch (error) {
        return { error: String(error?.message ?? error) };
      } finally {
        if (client) {
          await client.query('ROLLBACK').catch(() => {});
          client.release();
        }
      }
    },
    async close() {
      const open = pool;
      pool = null;
      await open?.end().catch(() => {});
    },
  };
}

const HOP = new Set(['connection', 'keep-alive', 'transfer-encoding', 'content-length', 'host', 'proxy-connection', 'te', 'trailer', 'upgrade']);

/**
 * One allow-listed real read, made by the twin's thread for both ghosts of a request. It must be a GET or HEAD to an address `realReads`
 * lists; it gets at most 5 s, and its answer is capped at maxBodyBytes. The answer comes back decoded, so it carries no content-encoding.
 * Answers { status, headers, body } or { error }.
 */
export async function realRead({ method, url, headers }, patterns, maxBodyBytes) {
  if (!readAllowed(patterns, method, url)) return { error: 'not an allowed real read' };
  try {
    const sent = Object.fromEntries(Object.entries(headers ?? {}).filter(([name]) => !HOP.has(name.toLowerCase())));
    const response = await fetch(url, { method, headers: sent, redirect: 'manual', signal: AbortSignal.timeout(5000) });
    const body = method === 'HEAD' ? Buffer.alloc(0) : await readCapped(response.body, maxBodyBytes);
    if (body === null) return { error: 'the answer was over the limit' };
    const answer = Object.fromEntries([...response.headers].filter(([name]) => name !== 'content-encoding' && name !== 'content-length'));
    return { status: response.status, headers: answer, body: exactBytes([body]) };
  } catch (error) {
    return { error: String(error?.message ?? error) };
  }
}
