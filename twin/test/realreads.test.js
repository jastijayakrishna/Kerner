import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pgReadable } from '../src/real-reads.js';
import { createTwin } from '../src/index.js';
import { app, listen, production } from './helpers.js';

test('a realReads address is a whole http or https address, so the origin it allows is plain', async () => {
  const versions = { old: app('rates', 'old'), candidate: app('rates', 'new') };
  for (const pattern of ['GET /rates/*', 'GET rates.example/*', 'GET file:///etc/*']) {
    await assert.rejects(createTwin({ ...versions, realReads: [pattern] }), /realReads addresses are whole http or https addresses/, pattern);
  }
});

test("pgReads takes a connection string, and needs the 'pg' adapter whose reads it answers", async () => {
  const versions = { old: app('rates', 'old'), candidate: app('rates', 'new') };
  await assert.rejects(createTwin({ ...versions, pgReads: 'postgres://reader@replica/app' }), /pgReads needs the 'pg' adapter/);
  await assert.rejects(createTwin({ ...versions, adapters: ['pg'], pgReads: 42 }), /pgReads must be a Postgres connection string/);
});

test('a Postgres query is made for real only when it is a read and calls nothing that acts even in a read-only transaction', () => {
  for (const query of ['select * from "access_approval_policies_approvers" where ("approverUserId" in ($1))', '  WITH t AS (SELECT 1) SELECT * FROM t', 'SHOW search_path', 'TABLE products']) {
    assert.equal(pgReadable(query), true, query);
  }
  for (const query of [
    'DELETE FROM membership_roles WHERE id = $1', 'WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x', "SELECT nextval('s')",
    'SELECT pg_advisory_lock(1)', 'select pg_try_advisory_xact_lock($1)', 'SELECT pg_sleep(10)', "SELECT dblink('x', 'y')", "SELECT pg_notify('c', 'p')",
    "SELECT set_config('a', 'b', false)", 'SELECT pg_terminate_backend(1)', 'SELECT * FROM t FOR UPDATE', 'COPY t TO STDOUT', 'BEGIN', 'SET search_path TO x',
  ]) assert.equal(pgReadable(query), false, query);
});

test('a query text that holds more than one statement is never made for real: a COMMIT inside it would end the read-only transaction', () => {
  // Without values, pg sends the text whole, and Postgres runs every statement in it: after the COMMIT, the DELETE would run outside the
  // read-only transaction. A comment marker inside a string literal must not hide what follows it.
  for (const query of ['SELECT 1; COMMIT; DELETE FROM users', "SELECT '--'; COMMIT; DELETE FROM users", "SELECT '/*'; END; DELETE FROM t; SELECT '*/'", 'select 1;\nend;\ndelete from t', 'SELECT 1; -- one more']) {
    assert.equal(pgReadable(query), false, query);
  }
  for (const query of ['SELECT 1;', 'SELECT 1 ;  \n', "SELECT 'a' AS b"]) assert.equal(pgReadable(query), true, query);
});

test('a read the real app answered from its warm cache is made for real by the twin, once, for both cold ghosts', async () => {
  let hits = 0;
  const rates = await listen((req, res) => {
    hits++;
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end('{"rate":1.1}');
  });
  process.env.KERN_TEST_RATES = rates.url;
  try {
    // The real app warms its cache on a request the twin does not sample.
    const warm = await production('rates', { sampleRate: 0 });
    assert.deepEqual(await (await fetch(`${warm.url}/`)).json(), { rate: 1.1 });
    await warm.close();
    assert.equal(hits, 1);
    // Fresh ghosts are cold: without real reads, their call is one the real run did not make.
    const cold = await production('rates');
    await (await fetch(`${cold.url}/`)).arrayBuffer();
    await cold.twin.idle();
    assert.equal(cold.twin.stats().notCompared, 1, JSON.stringify(cold.twin.stats()));
    await cold.close();
    assert.equal(hits, 1);
    // With the address allow-listed, the twin's thread makes the read for both ghosts.
    const allowed = await production('rates', { realReads: [`GET ${rates.url}/*`] });
    await (await fetch(`${allowed.url}/`)).arrayBuffer();
    await allowed.twin.idle();
    assert.equal(allowed.twin.stats().same, 1, JSON.stringify(allowed.twin.stats()));
    assert.equal(hits, 2, 'one real read, shared by both ghosts');
    await allowed.close();
  } finally {
    delete process.env.KERN_TEST_RATES;
    await rates.close();
  }
});
