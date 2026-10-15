import { createRequire } from 'node:module';
import { readCapped } from './record.js';
import { exactBytes } from './serialize.js';
import { pgIsWrite, readAllowed } from './keys.js';
import { resultOf } from './adapters/pg.js';

const PG_READ = /^\s*(select|with|table|values|show)\b/i;
const PG_ACTS = /\b(pg_(try_)?advisory\w*|pg_sleep\w*|dblink\w*|lo_\w+|pg_notify|set_config|pg_terminate_backend|pg_cancel_backend|pg_reload_conf|pg_rotate_logfile|pg_file_write|pg_read_file|pg_read_binary_file|pg_ls_dir|nextval|setval|currval)\s*\(|\bfor\s+(update|no\s+key\s+update|share|key\s+share)\b/i;

const MORE_STATEMENTS = /;\s*\S/;

export function pgReadable(text) {
  if (MORE_STATEMENTS.test(String(text))) return false;
  const query = String(text).replace(/--[^\n]*|\/\*[\s\S]*?\*\//g, ' ');
  return PG_READ.test(query) && !pgIsWrite(query) && !PG_ACTS.test(query);
}

export function pgReader(url, appModule) {
  let pool = null;
  return {
    async read(text, values) {
      if (!pgReadable(text)) return { error: 'not a query Kern makes for real' };
      let client = null;
      try {
        if (!pool) {
          pool = new (createRequire(appModule)('pg').Pool)({ connectionString: url, max: 2 });
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

export async function realRead({ method, url, headers }, patterns, maxBodyBytes) {
  if (!readAllowed(patterns, method, url)) return { error: 'not an allowed real read' };
  try {
    const sent = Object.fromEntries(Object.entries(headers ?? {}).filter(([name]) => !HOP.has(name.toLowerCase())));
    const response = await fetch(url, { method, headers: sent, redirect: 'manual', signal: AbortSignal.timeout(5000) });
    const body = method === 'HEAD' ? Buffer.alloc(0) : await readCapped(response.body, maxBodyBytes);
    if (body === null) return { error: 'the answer was over the limit' };
    const answer = Object.fromEntries([...response.headers].filter(([name]) => name !== 'content-encoding' && name !== 'content-length'));
    const cookies = response.headers.getSetCookie();
    if (cookies.length) answer['set-cookie'] = cookies;
    return { status: response.status, headers: answer, body: exactBytes([body]) };
  } catch (error) {
    return { error: String(error?.message ?? error) };
  }
}
