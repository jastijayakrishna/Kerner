import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Worker } from 'node:worker_threads';
import { wrapListener } from '../src/capture.js';
import { exactBytes } from '../src/serialize.js';
import { listen, post } from './helpers.js';

test('a sampled request is handed over as bytes, and only its own bytes', async () => {
  let copy;
  const listener = wrapListener((req, res) => {
    req.resume();
    req.on('end', () => res.end('ok'));
  }, {
    sample: () => true,
    maxBodyBytes: 1024,
    onError: (error) => {
      throw error;
    },
    deliver: (promise) => {
      copy = promise;
    },
  });
  const server = await listen(listener);
  await (await post(server.url, { a: 1 })).text();
  const capture = await copy;
  await server.close();
  for (const bytes of [capture.request.body, capture.response.body]) {
    assert.ok(bytes instanceof Uint8Array, 'bytes, not text');
    assert.equal(bytes.buffer.byteLength, bytes.byteLength, 'an exact copy, never a slice of a shared buffer');
  }
  const echo = new Worker(
    "const { parentPort } = require('node:worker_threads'); parentPort.on('message', (m) => parentPort.postMessage(Buffer.from(m.request.body.buffer).toString('latin1')));",
    { eval: true },
  );
  const seen = await new Promise((resolve) => {
    echo.once('message', resolve);
    echo.postMessage(capture);
  });
  await echo.terminate();
  assert.equal(seen, '{"a":1}', 'the thread got these bytes and nothing else');
});

test('exactBytes joins parts into one array of exactly their size', () => {
  const joined = exactBytes([Buffer.from('ab'), new Uint8Array([99])]);
  assert.equal(Buffer.from(joined).toString(), 'abc');
  assert.equal(joined.buffer.byteLength, 3);
});
