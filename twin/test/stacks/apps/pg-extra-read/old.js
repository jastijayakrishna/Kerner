import pg from 'pg';

// A plan page, and a health route, on Postgres.
export function createApp() {
  const pool = new pg.Pool({ connectionString: process.env.KERN_TWIN_PG_URL, max: 2 });
  const send = (res, value) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(value));
  };
  return async (req, res) => {
    if (req.url === '/plan') {
      const { rows } = await pool.query('SELECT name FROM plans WHERE id = $1', [1]);
      return send(res, { plan: rows[0].name });
    }
    const { rows } = await pool.query('SELECT 1 AS ok');
    return send(res, { ok: rows[0].ok === 1 });
  };
}
