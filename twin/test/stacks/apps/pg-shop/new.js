import pg from 'pg';
import { createApp as createOld } from './old.js';

export function createApp() {
  const old = createOld();
  const pool = new pg.Pool({ connectionString: process.env.KERN_TWIN_PG_URL, max: 2 });
  return async (req, res) => {
    if (req.method === 'GET' && req.url === '/products') {
      const { rows } = await pool.query('SELECT id, name, price_cents, stock FROM products ORDER BY id');
      res.writeHead(200, { 'content-type': 'application/json' });
      return res.end(JSON.stringify(rows.map((p) => ({ id: p.id, name: p.name, price: p.price_cents / 100, stock: p.stock }))));
    }
    return old(req, res);
  };
}
