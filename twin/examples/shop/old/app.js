import express from 'express';
import pg from 'pg';

// A small shop on Express and Postgres, as the twin would meet one in production. createApp builds the app and starts nothing.
export function createApp() {
  const pool = new pg.Pool({ connectionString: process.env.DATABASE_URL, max: 4 });
  const app = express();
  app.use(express.json());
  const product = (row) => ({ id: row.id, name: row.name, price: row.price_cents / 100, stock: row.stock });

  app.get('/products', async (req, res) => {
    const { rows } = await pool.query('SELECT id, name, price_cents, stock FROM products ORDER BY id');
    res.json(rows.map(product));
  });

  app.get('/products/:id', async (req, res) => {
    const { rows } = await pool.query('SELECT id, name, price_cents, stock FROM products WHERE id = $1', [Number(req.params.id)]);
    if (!rows.length) return res.status(404).json({ error: 'no such product' });
    res.json(product(rows[0]));
  });

  app.post('/orders', async (req, res) => {
    const { productId, quantity } = req.body ?? {};
    if (!Number.isInteger(productId) || !Number.isInteger(quantity) || quantity < 1) return res.status(400).json({ error: 'productId and quantity, please' });
    const client = await pool.connect();
    try {
      await client.query('BEGIN');
      const taken = await client.query('UPDATE products SET stock = stock - $1 WHERE id = $2 AND stock >= $1 RETURNING price_cents', [quantity, productId]);
      if (!taken.rows.length) {
        await client.query('ROLLBACK');
        return res.status(409).json({ error: 'not enough stock' });
      }
      const total = taken.rows[0].price_cents * quantity;
      const order = await client.query('INSERT INTO orders (product_id, quantity, total_cents, created_at) VALUES ($1, $2, $3, $4) RETURNING id', [productId, quantity, total, new Date()]);
      await client.query('COMMIT');
      res.status(201).json({ id: order.rows[0].id, total: total / 100 });
    } catch (error) {
      await client.query('ROLLBACK');
      throw error;
    } finally {
      client.release();
    }
  });

  app.get('/orders/:id', async (req, res) => {
    const { rows } = await pool.query('SELECT id, product_id, quantity, total_cents FROM orders WHERE id = $1', [Number(req.params.id)]);
    if (!rows.length) return res.status(404).json({ error: 'no such order' });
    const [order] = rows;
    res.json({ id: order.id, productId: order.product_id, quantity: order.quantity, total: order.total_cents / 100 });
  });

  return app;
}
