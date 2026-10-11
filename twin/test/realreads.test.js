import { test } from 'node:test';
import assert from 'node:assert/strict';
import { listen, production } from './helpers.js';

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
