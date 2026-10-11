CREATE TABLE products (id serial PRIMARY KEY, name text NOT NULL, price_cents integer NOT NULL, stock integer NOT NULL);
CREATE TABLE orders (id serial PRIMARY KEY, product_id integer NOT NULL REFERENCES products (id), quantity integer NOT NULL,
  total_cents integer NOT NULL, created_at timestamptz NOT NULL);
INSERT INTO products (name, price_cents, stock) SELECT 'product ' || n, 199 + n * 37, 50 FROM generate_series(1, 10) AS n;
