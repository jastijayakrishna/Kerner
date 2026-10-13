import { test } from 'node:test';
import assert from 'node:assert/strict';
import { callKey, pgKey } from '../src/keys.js';
import { scrubCapture, scrubber } from '../src/scrub.js';

const luhn = (digits) => {
  let sum = 0;
  [...digits].reverse().forEach((d, i) => {
    let n = Number(d);
    if (i % 2) n = n * 2 > 9 ? n * 2 - 9 : n * 2;
    sum += n;
  });
  return sum % 10 === 0;
};

test('private values get stand-ins of the same shape, the same value always the same stand-in within one scrubber', () => {
  const s = scrubber();
  const text = s.text('mail ada@corp.com, card 4111 1111 1111 1111, auth Bearer sk_live_0123456789abcdefghij0123456789ab, mail ada@corp.com again');
  assert.doesNotMatch(text, /ada@corp\.com|4111 1111 1111 1111|sk_live_0123456789abcdefghij0123456789ab/);
  const emails = text.match(/user-[0-9a-f]{8}@example\.com/g);
  assert.equal(emails.length, 2);
  assert.equal(emails[0], emails[1], 'one value, one stand-in');
  const card = text.match(/card (\d{4} \d{4} \d{4} \d{4})/)[1];
  assert.ok(luhn(card.replace(/ /g, '')), `${card} is a valid card number`);
  assert.match(card, /^4/);
  assert.match(text, /Bearer [a-z]{2}_[a-z]{4}_[0-9a-z]{32}\b/, 'letters for letters, digits for digits, the underscores kept');
  assert.deepEqual(s.value({ password: 'hunter22', nested: { api_key: 'AbC123', note: 'ok' }, list: ['bob@x.org'] }).nested.note, 'ok');
  const scrubbed = s.value({ password: 'hunter22', nested: { api_key: 'AbC123' } });
  assert.match(scrubbed.password, /^[a-z]{6}\d{2}$/);
  assert.match(scrubbed.nested.api_key, /^[A-Z][a-z][A-Z]\d{3}$/);
  assert.notEqual(scrubber().text('ada@corp.com'), scrubber().text('ada@corp.com'), 'nothing links two scrubbers');
});

test('a capture is scrubbed everywhere a private value can be, and every recorded call is named again from its scrubbed payload', () => {
  const sent = Buffer.from(JSON.stringify({ email: 'ada@corp.com' }));
  const capture = {
    start: 1, seed: 2, route: 'POST /checkout', segment: 'all', serving: 'old',
    request: { method: 'POST', url: '/checkout?email=ada%40corp.com', headers: { authorization: 'Bearer abc', 'content-type': 'application/json' }, body: new Uint8Array(Buffer.from('{"email":"ada@corp.com","card":"4111111111111111"}')) },
    response: { status: 200, headers: { 'content-type': 'application/json' }, body: new Uint8Array(Buffer.from('{"receipt":"ada@corp.com"}')) },
    io: [
      { kind: 'fetch', key: callKey('POST', 'https://crm.example/people', sent), method: 'POST', url: 'https://crm.example/people', write: true, sent: new Uint8Array(sent), status: 200, headers: { 'content-type': 'application/json' }, body: new Uint8Array(Buffer.from('{"id":7,"email":"ada@corp.com"}')), error: null },
      { kind: 'pg', key: pgKey('SELECT * FROM people WHERE email = $1', ['ada@corp.com']), write: false, text: 'SELECT * FROM people WHERE email = $1', values: ['ada@corp.com'], result: { command: 'SELECT', rowCount: 1, rows: [{ email: 'ada@corp.com' }], fields: [] }, error: null },
    ],
    effects: [],
  };
  const { capture: out, opaque } = scrubCapture(capture, {});
  const all = JSON.stringify(out, (key, value) => (value instanceof Uint8Array ? Buffer.from(value).toString() : value));
  assert.doesNotMatch(all, /ada@corp\.com|ada%40corp\.com|4111111111111111|Bearer abc/, 'the real answer included: nothing in a scrubbed capture keeps a real value');
  assert.equal(out.request.headers.authorization, '[scrubbed]');
  assert.deepEqual(opaque, []);
  const email = JSON.parse(Buffer.from(out.request.body).toString()).email;
  assert.equal(JSON.parse(Buffer.from(out.io[0].sent).toString()).email, email, 'the same stand-in in the request and in the call it led to');
  assert.equal(out.io[0].key, callKey('POST', 'https://crm.example/people', out.io[0].sent));
  assert.equal(out.io[1].key, pgKey('SELECT * FROM people WHERE email = $1', [email]));
  assert.deepEqual(out.effects, [out.io[0].key]);
});

test('an address or a form body is scrubbed piece by piece: order, repeated names and the way each piece was written are kept', () => {
  const s = scrubber();
  const address = s.url('/people/ada%40corp.com?tag=a&tag=b+c&email=ada@corp.com&token=abc123&q=x%20y#top');
  const [, inPath, inQuery, token] = /^\/people\/(user-[0-9a-f]{8})%40example\.com\?tag=a&tag=b\+c&email=(user-[0-9a-f]{8})@example\.com&token=([a-f]{3}\d{3})&q=x%20y#top$/.exec(address) ?? [];
  assert.ok(inPath, address);
  assert.equal(inPath, inQuery, 'one value, one stand-in, however it was written');
  assert.notEqual(token, 'abc123', 'a value under a private name is always replaced');
  assert.equal(s.url('https://crm.example/v1/people?limit=10&sort=-name'), 'https://crm.example/v1/people?limit=10&sort=-name', 'nothing private: kept exactly');
  const form = s.body(new Uint8Array(Buffer.from('user=ada&email=ada%40corp.com&password=hunter2&remember=on')), { 'content-type': 'application/x-www-form-urlencoded' });
  assert.match(Buffer.from(form.bytes).toString(), /^user=ada&email=user-[0-9a-f]{8}%40example\.com&password=[a-z]{6}\d&remember=on$/);
  assert.equal(form.opaque, false);
});

test('a UUID stays a valid UUID, a hex value stays hex, and the content type is kept, so the app can still read what it is given', () => {
  const s = scrubber();
  const id = s.text('123e4567-e89b-42d3-a456-426614174000');
  assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-a[0-9a-f]{3}-[0-9a-f]{12}$/, 'its version and variant kept');
  assert.notEqual(id, '123e4567-e89b-42d3-a456-426614174000');
  assert.equal(s.text('123E4567-E89B-42D3-A456-426614174000'), id.toUpperCase(), 'one value, one stand-in, in either case');
  const hash = '9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08';
  assert.match(s.text(hash), /^[0-9a-f]{64}$/);
  assert.notEqual(s.text(hash), hash);
  const type = 'multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW1234';
  const headers = s.headersOf({ 'content-type': type, 'x-trace': 'WebKitFormBoundary7MA4YWxkTrZu0gW1234' });
  assert.equal(headers['content-type'], type, "a file upload's parts are found by the boundary it names");
  assert.notEqual(headers['x-trace'], 'WebKitFormBoundary7MA4YWxkTrZu0gW1234', 'other headers are still scrubbed');
});

test('scrubbing takes time in proportion to the text, so a large body cannot stall the twin', () => {
  const s = scrubber();
  const started = performance.now();
  for (const text of ['a'.repeat(1 << 17), 'a-'.repeat(1 << 16), '1 '.repeat(1 << 16)]) s.text(text);
  // A few milliseconds; a search that rescans the text from every place in it takes about 20 seconds here.
  const ms = performance.now() - started;
  assert.ok(ms < 2000, `took ${Math.round(ms)} ms`);
});

test('a body nested a hundred thousand deep is scrubbed in full, private names included, without running out of stack', () => {
  // JSON.parse reads any depth. A scrubber that gave up on it would fall back to patterns alone, and a value under a private name
  // (a password) would reach the disk as it was.
  const deep = `${'['.repeat(100000)}"ada@corp.com"${']'.repeat(100000)}`;
  const body = new Uint8Array(Buffer.from(`{"password":"hunter2","deep":${deep}}`));
  const reply = JSON.parse(deep);
  const capture = {
    start: 1, seed: 2, route: 'POST /x', segment: 'all', serving: 'old',
    request: { method: 'POST', url: '/x', headers: { 'content-type': 'application/json' }, body },
    response: { status: 200, headers: {}, body: new Uint8Array() },
    io: [{ kind: 'redis', key: 'REDIS EVALSHA - -', write: true, command: 'evalsha', args: [], reply, error: null }],
    effects: [],
  };
  const { capture: out, opaque } = scrubCapture(capture, {});
  assert.deepEqual(opaque, []);
  const text = Buffer.from(out.request.body).toString();
  assert.doesNotMatch(text, /hunter2|ada@corp\.com/);
  assert.match(text, /^\{"password":"[a-z]{6}\d","deep":\[{100000}"user-[0-9a-f]{8}@example\.com"\]{100000}\}$/);
  let inner = out.io[0].reply;
  for (let i = 0; i < 100000; i++) inner = inner[0];
  assert.match(inner, /^user-[0-9a-f]{8}@example\.com$/);
});
