import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../../src/index.js';
import { pgReader } from '../../src/real-reads.js';
import { listen, post } from '../helpers.js';

// Against real Postgres and Redis: run by scripts/test-stacks.sh, which starts both and sets these.
const PG = process.env.KERN_TWIN_PG_URL;
const REDIS = process.env.KERN_TWIN_REDIS_URL;
const stackApp = (name, version) => ({ module: fileURLToPath(new URL(`./apps/${name}/${version}.js`, import.meta.url)), export: 'createApp' });

async function stack(name, options) {
  const twin = await createTwin({ old: stackApp(name, 'old'), candidate: stackApp(name, 'new'), timeoutMs: 5000, ...options });
  const server = await listen(await twin.listener());
  return {
    twin,
    url: server.url,
    close: async () => {
      await server.close();
      await twin.close();
    },
  };
}

test('Postgres queries are replayed to the ghosts, a transaction and a clock-stamped insert included, and nothing is written twice', { skip: !PG && 'needs Postgres: run scripts/test-stacks.sh' }, async () => {
  const { default: pg } = await import('pg');
  const admin = new pg.Client({ connectionString: PG });
  await admin.connect();
  await admin.query(`DROP TABLE IF EXISTS orders; DROP TABLE IF EXISTS products;
    CREATE TABLE products (id serial PRIMARY KEY, name text, price_cents int, stock int);
    CREATE TABLE orders (id serial PRIMARY KEY, product_id int, quantity int, total_cents int, created_at timestamptz);
    INSERT INTO products (name, price_cents, stock) VALUES ('tea', 450, 10), ('cake', 300, 2);`);
  // Started inside the try: a twin that fails to start must still let the admin connection close, or the run never ends.
  let shop = null;
  try {
    shop = await stack('pg-shop', { adapters: ['pg'] });
    assert.deepEqual(shop.twin.stats().adapters, { fetch: 'recording', pg: 'recording' });
    assert.deepEqual(await (await fetch(`${shop.url}/products`)).json(), [
      { id: 1, name: 'tea', price: '4.50', stock: 10 },
      { id: 2, name: 'cake', price: '3.00', stock: 2 },
    ]);
    assert.equal((await post(`${shop.url}/orders`, { productId: 1, quantity: 2 })).status, 201);
    assert.equal((await post(`${shop.url}/orders`, { productId: 2, quantity: 5 })).status, 409);
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.same, stats.different, stats.notCompared, stats.skipped], [2, 1, 0, 0], JSON.stringify(stats));
    assert.deepEqual(shop.twin.differences()[0].details[0], { kind: 'body', path: '$[0].price', old: '"4.50"', new: '4.5' });
    const { rows } = await admin.query('SELECT (SELECT count(*) FROM orders)::int AS orders, (SELECT stock FROM products WHERE id = 1) AS stock');
    assert.deepEqual(rows[0], { orders: 1, stock: 8 }, 'only the real app wrote');
  } finally {
    await shop?.close();
    await admin.end();
  }
});

test('with pgReads, a read the new code makes and the old never did is answered for real and read-only; a write never is', { skip: !PG && 'needs Postgres: run scripts/test-stacks.sh' }, async () => {
  const { default: pg } = await import('pg');
  const admin = new pg.Client({ connectionString: PG });
  await admin.connect();
  await admin.query(`DROP TABLE IF EXISTS plan_features; DROP TABLE IF EXISTS plans; DROP SEQUENCE IF EXISTS plan_seq;
    CREATE TABLE plans (id int PRIMARY KEY, name text); CREATE TABLE plan_features (plan_id int, feature text); CREATE SEQUENCE plan_seq;
    INSERT INTO plans VALUES (1, 'pro'); INSERT INTO plan_features VALUES (1, 'sso'), (1, 'audit');`);
  let shop = null;
  try {
    // Without pgReads, the new code's extra read has no answer: the request is not compared.
    shop = await stack('pg-extra-read', { adapters: ['pg'] });
    assert.deepEqual(await (await fetch(`${shop.url}/plan`)).json(), { plan: 'pro' });
    await shop.twin.idle();
    assert.equal(shop.twin.stats().notCompared, 1, JSON.stringify(shop.twin.stats()));
    await shop.close();
    // With pgReads, the twin's thread answers it for real, read-only, for both ghosts.
    shop = await stack('pg-extra-read', { adapters: ['pg'], pgReads: PG });
    assert.deepEqual(await (await fetch(`${shop.url}/plan`)).json(), { plan: 'pro' });
    assert.deepEqual(await (await fetch(`${shop.url}/health`)).json(), { ok: true });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.different, stats.notCompared], [2, 0], JSON.stringify(stats));
    const [plan, health] = shop.twin.differences();
    assert.deepEqual(plan.details[0], { kind: 'body', path: '$.features', old: 'missing', new: '2' }, 'the real read answered the new code');
    assert.deepEqual(health.kinds, ['effects'], "the new code's nextval is a write: never made for real, so it is the difference");
    const { rows } = await admin.query('SELECT is_called FROM plan_seq');
    assert.equal(rows[0].is_called, false, 'the sequence never moved');
  } finally {
    await shop?.close();
    await admin.end();
  }
});

test('Redis commands are replayed to the ghosts, pipelines and transformed replies included, and nothing is counted twice', { skip: !REDIS && 'needs Redis: run scripts/test-stacks.sh' }, async () => {
  const { default: Redis } = await import('ioredis');
  const admin = new Redis(REDIS);
  await admin.del('visits');
  await admin.set('greeting', 'hi');
  await admin.hset('profile', 'plan', 'pro');
  let shop = null;
  try {
    shop = await stack('redis-cache', { adapters: ['ioredis'] });
    assert.deepEqual(await (await fetch(`${shop.url}/hello`)).json(), { greeting: 'hi' });
    assert.deepEqual(await (await fetch(`${shop.url}/visit`)).json(), { visits: 1, greeting: 'hi', plan: 'pro' });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.same, stats.different, stats.notCompared], [1, 1, 0], JSON.stringify(stats));
    assert.ok(shop.twin.differences()[0].kinds.includes('effects'), JSON.stringify(shop.twin.differences()));
    assert.equal(await admin.get('visits'), '1', 'only the real app counted');
  } finally {
    await shop?.close();
    admin.disconnect();
  }
});

test('a Postgres real read is one statement: a COMMIT inside its text never lets the rest of it write', { skip: !PG && 'needs Postgres: run scripts/test-stacks.sh' }, async () => {
  const { default: pg } = await import('pg');
  const admin = new pg.Client({ connectionString: PG });
  await admin.connect();
  await admin.query('DROP TABLE IF EXISTS ledger; CREATE TABLE ledger (id int); INSERT INTO ledger VALUES (1);');
  const reader = pgReader(PG, stackApp('pg-extra-read', 'old').module);
  try {
    for (const text of ['SELECT 1; COMMIT; DELETE FROM ledger', "SELECT '--'; COMMIT; DELETE FROM ledger"]) {
      const answer = await reader.read(text, []);
      assert.ok(answer.error, `${text}: ${JSON.stringify(answer)}`);
    }
    const { rows } = await admin.query('SELECT count(*)::int AS n FROM ledger');
    assert.equal(rows[0].n, 1, 'the row is still there');
  } finally {
    await reader.close();
    await admin.end();
  }
});
