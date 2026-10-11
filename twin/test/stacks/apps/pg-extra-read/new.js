import pg from 'pg';

// The new version reads the plan's features too, which the old one never read; and its health route moves a sequence, a write disguised
// as a SELECT.
export function createApp() {
  const pool = new pg.Pool({ connectionString: process.env.KERN_TWIN_PG_URL, max: 2 });
  const send = (res, value) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(value));
  };
  return async (req, res) => {
    if (req.url === '/plan') {
      const { rows } = await pool.query('SELECT name FROM plans WHERE id = $1', [1]);
      const features = await pool.query('SELECT count(*)::int AS n FROM plan_features WHERE plan_id = $1', [1]);
      return send(res, { plan: rows[0].name, features: features.rows[0].n });
    }
    const { rows } = await pool.query("SELECT nextval('plan_seq') AS n");
    return send(res, { ok: rows[0].n > 0 });
  };
}
