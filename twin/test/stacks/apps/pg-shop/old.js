import pg from 'pg';

const send = (res, status, value) => {
  const body = JSON.stringify(value);
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(body);
};
const readJson = async (req) => {
  const parts = [];
  for await (const part of req) parts.push(part);
  return parts.length ? JSON.parse(Buffer.concat(parts).toString()) : {};
};

export function createApp() {
  const pool = new pg.Pool({ connectionString: process.env.KERN_TWIN_PG_URL, max: 2 });
  return async (req, res) => {
    if (req.method === 'GET' && req.url === '/products') {
      const { rows } = await pool.query('SELECT id, name, price_cents, stock FROM products ORDER BY id');
      return send(res, 200, rows.map((p) => ({ id: p.id, name: p.name, price: (p.price_cents / 100).toFixed(2), stock: p.stock })));
    }
    if (req.method === 'POST' && req.url === '/orders') {
      const { productId, quantity } = await readJson(req);
      const client = await pool.connect();
      try {
        await client.query('BEGIN');
        const taken = await client.query('UPDATE products SET stock = stock - $1 WHERE id = $2 AND stock >= $1 RETURNING price_cents', [quantity, productId]);
        if (!taken.rows.length) {
          await client.query('ROLLBACK');
          return send(res, 409, { error: 'not enough stock' });
        }
        const order = await client.query('INSERT INTO orders (product_id, quantity, total_cents, created_at) VALUES ($1, $2, $3, $4) RETURNING id', [productId, quantity, taken.rows[0].price_cents * quantity, new Date()]);
        await client.query('COMMIT');
        return send(res, 201, { id: order.rows[0].id });
      } finally {
        client.release();
      }
    }
    return send(res, 404, { error: 'no such route' });
  };
}
