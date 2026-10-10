# Twin runtime, Phase 1 (shadow twin core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This user allows at most 2 helper agents per task, so superpowers:executing-plans (inline, with checkpoints) fits best.

**Goal:** A Node library that runs the old and the new version of a service as harmless ghosts on copies of real requests, and reports every difference, without ever changing what a customer gets.

**Architecture:**
- The real app keeps serving.
- A thin wrapper around its request listener copies sampled requests, their answers and their `fetch` calls.
- After the answer has gone out, two worker threads (the old and the new version) replay the copy through real HTTP on loopback, in one sealed world: fixed clock, seeded randomness, fetch answered from the recording, writes written down, everything else refused.
- The old ghost must reproduce the real status and writes. Then the two ghosts are compared.

**Tech Stack:** Node 22 or newer. ESM, `node:test`, `worker_threads`, `async_hooks`, `zlib`, `crypto`. No dependencies. Docker runs the Linux tests in the pinned image `docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392`.

The design, with its rules, failure table, maths and later phases, is in `docs/superpowers/specs/2026-10-10-twin-runtime-design.md`. Read it first.

---

## Read this first

- **Kern.** A Rust tool in this repository that checks AI-written code changes (`README.md`). The twin is its next product: it proves a change on real requests before customers see it. Phase 1 builds the comparing core. Switching versions on (Phase 2), naming the changed code (Phase 3), real databases (Phase 4), the fix loop (Phase 5) and proof on real apps (Phase 6) come later, each with its own plan.
- **The user's rules:**
  - Answer in simple English.
  - Use at most 2 helper agents per task.
  - Ask before any commit or push.
  - Test on Linux first.
  - Never commit `research_notes/` or `reports/`.
- **The repository at plan time:**
  - Branch `kern-drive`. At the user's request on 2026-10-10, the `kern drive` work and this plan were committed there, locally. Nothing is pushed.
  - Task 0 creates the `twin` branch from it.
  - Main branch is `master`.
- **The machine:**
  - Windows has Node 22.12 (`node`).
  - WSL's Node is 18, which is too old. Linux tests run in Docker inside WSL from `/mnt/c/Users/krish/Downloads/Kern`, through `twin/scripts/test-linux.sh` (Task 1).
- **Style:**
  - Match Kern's code: short plain-English comments that say why.
  - Small files with one job each.
  - No dependencies.
  - Every hook fails open for the customer and fails closed for the comparison.
- **Test commands used in every task:**
  - Windows, from the repository root: `node --test --test-concurrency=1 twin/test/<file>.test.js`.
  - Linux, from the repository root in WSL: `sh twin/scripts/test-linux.sh test/<file>.test.js`. With no argument it runs every test.
- **Commits.** Every commit message ends with the attribution line your session's instructions give. At plan time that was `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## File map

| File | Job |
|---|---|
| `twin/package.json` | Package metadata; `npm test` runs every test |
| `twin/scripts/test-linux.sh` | Runs tests in the pinned Node 22 image |
| `twin/src/keys.js` | Names an outgoing call the same way everywhere; says whether a method writes |
| `twin/src/serialize.js` | Unpacks bodies, reads JSON or text by value, lists differing paths |
| `twin/src/compare.js` | Compares two runs; checks that the old ghost reproduced reality |
| `twin/src/world.js` | The ghost's fixed clock and seeded random numbers |
| `twin/src/ghost-io.js` | What a ghost may do: replay fetch, write down writes, refuse the rest |
| `twin/src/worker.js` | One ghost: one version of the app, served on loopback, answering jobs |
| `twin/src/coordinator.js` | The ghost pair: start, ask, time out, restart together, judge, count |
| `twin/src/capture.js` | Wraps the real listener; copies what comes in and goes out |
| `twin/src/record.js` | Records the real app's `fetch` calls for the sampled request |
| `twin/src/index.js` | `createTwin()`, the public API |
| `twin/test/helpers.js` | Test helpers: app paths, servers, a production-like setup |
| `twin/test/*.test.js` | The tests |
| `twin/test/apps/**` | Small old and new versions of apps, one folder per scenario |
| `twin/bench/overhead.js` | Measures what the twin adds to request latency |
| `twin/README.md` | How to use it, what it promises, what it does not do yet |
| `.github/workflows/ci.yml` | Runs the twin tests on Linux in CI |

---

### Task 0: Get ready

**Files:** none changed until the user agrees.

- [ ] **Step 1: Read the design and check the starting point**

Read `docs/superpowers/specs/2026-10-10-twin-runtime-design.md`. Then run:

```bash
git branch --show-current
git log --oneline -3
git status --short
```

Expected:
- The branch is `kern-drive`.
- The latest two commits are "Design and plan the twin runtime" and "Add kern drive: run every changed line on the old and the new code".
- Only `research_notes/` and `reports/` are untracked.

If the commits are missing, stop and ask the user how to proceed.

- [ ] **Step 2: Ask the user once**

Ask, in simple English:

> "May I create a twin branch and commit each finished task there? Everything stays local; nothing is pushed."

Wait for a clear yes. If the answer is no, do every task without the commit steps.

- [ ] **Step 3: Create the twin branch (only after yes)**

```bash
git switch -c twin
```

Expected: `Switched to a new branch 'twin'`.

- [ ] **Step 4: Check the tools**

```bash
node --version
wsl -e docker version --format "{{.Server.Version}}"
```

Expected: `v22.12.0` or newer, then a Docker server version. If Docker does not answer in WSL, ask the user to start it. Every task can still be tested on Windows meanwhile.

---

### Task 1: Package, call names and reading values

**Files:**
- Create: `twin/package.json`, `twin/scripts/test-linux.sh`, `twin/src/keys.js`, `twin/src/serialize.js`
- Test: `twin/test/serialize.test.js`

- [ ] **Step 1: Create the package and the Linux test script**

`twin/package.json`:

```json
{
  "name": "kern-twin",
  "version": "0.1.0",
  "private": true,
  "description": "Runs a new version of a service as a harmless ghost beside the old one on copies of real requests, and reports every difference",
  "type": "module",
  "exports": { ".": "./src/index.js" },
  "engines": { "node": ">=22" },
  "scripts": { "test": "node --test --test-concurrency=1 \"test/**/*.test.js\"" }
}
```

`twin/scripts/test-linux.sh`:

```sh
#!/bin/sh
# Runs the twin's tests on Linux in the pinned Node 22 image (WSL's own Node is too old).
# Usage, from the repository root: sh twin/scripts/test-linux.sh [test files, relative to twin/]
set -eu
cd "$(dirname "$0")/.."
IMAGE=docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392
if [ "$#" -eq 0 ]; then set -- 'test/**/*.test.js'; fi
exec docker run --rm -v "$PWD:/twin" -w /twin "$IMAGE" node --test --test-concurrency=1 "$@"
```

- [ ] **Step 2: Write the failing test**

`twin/test/serialize.test.js`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { gzipSync } from 'node:zlib';
import { diffValues, readBody, preview } from '../src/serialize.js';
import { callKey, isWrite, digest } from '../src/keys.js';

test('differences are named by their path', () => {
  const found = diffValues({ total: 4051, items: [1, 2], name: 'a' }, { total: 4050.9, items: [1, 3], extra: true, name: 'a' });
  assert.deepEqual(found, [
    { path: '$.extra', old: 'missing', new: 'true' },
    { path: '$.items[1]', old: '2', new: '3' },
    { path: '$.total', old: '4051', new: '4050.9' },
  ]);
});

test('equal values have no differences, and names from the prototype do not count as present', () => {
  assert.deepEqual(diffValues({ a: [1, { b: null }] }, { a: [1, { b: null }] }), []);
  assert.deepEqual(diffValues({}, { constructor: 1 }), [{ path: '$.constructor', old: 'missing', new: '1' }]);
});

test('a compressed JSON body is read by value, text as text, anything else by its hash', () => {
  const body = gzipSync(Buffer.from(JSON.stringify({ ok: true })));
  assert.deepEqual(readBody(body, { 'content-type': 'application/json', 'content-encoding': 'gzip' }), { kind: 'json', value: { ok: true } });
  assert.equal(readBody(Buffer.from('<p>hi</p>'), { 'content-type': 'text/html' }).kind, 'text');
  assert.equal(readBody(Buffer.from([1, 2, 3]), {}).kind, 'bytes');
});

test('long values are cut short for showing', () => {
  assert.equal(preview('x'.repeat(500)).length, 120);
});

test('a call has the same name everywhere, and only reads are not writes', () => {
  assert.equal(callKey('post', 'https://pay.example/charge', Buffer.from('{"amount":5}')), `POST https://pay.example/charge ${digest('{"amount":5}').slice(0, 16)}`);
  assert.equal(callKey('GET', 'https://api.example/x', Buffer.alloc(0)), 'GET https://api.example/x -');
  assert.equal(isWrite('GET'), false);
  assert.equal(isWrite('delete'), true);
});
```

- [ ] **Step 3: Run it and see it fail**

Run: `node --test --test-concurrency=1 twin/test/serialize.test.js`

Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `twin/src/serialize.js`.

- [ ] **Step 4: Write the code**

`twin/src/keys.js`:

```js
import { createHash } from 'node:crypto';

/** Methods that only read; every other method changes something somewhere. */
export const READS = new Set(['GET', 'HEAD', 'OPTIONS']);

export function digest(data) {
  return createHash('sha256').update(data).digest('hex');
}

/** One outgoing call, named the same way in the real run and in both ghosts: method, address, and a short hash of the body. */
export function callKey(method, url, body) {
  const verb = String(method).toUpperCase();
  return `${verb} ${url} ${body && body.length ? digest(body).slice(0, 16) : '-'}`;
}

export function isWrite(method) {
  return !READS.has(String(method).toUpperCase());
}
```

`twin/src/serialize.js`:

```js
import { gunzipSync, inflateSync, brotliDecompressSync } from 'node:zlib';
import { digest } from './keys.js';

const MAX_DECODED = 16 << 20;

/** The body as the client reads it: compressed bodies are unpacked, up to 16 MiB. */
export function decodeBody(body, headers) {
  const encoding = String(headers['content-encoding'] ?? '').toLowerCase().trim();
  const options = { maxOutputLength: MAX_DECODED };
  if (encoding === 'gzip' || encoding === 'x-gzip') return gunzipSync(body, options);
  if (encoding === 'deflate') return inflateSync(body, options);
  if (encoding === 'br') return brotliDecompressSync(body, options);
  return body;
}

/** JSON and text bodies are compared by value; anything else by a hash of its bytes. */
export function readBody(body, headers) {
  let bytes;
  try {
    bytes = decodeBody(body, headers);
  } catch {
    return { kind: 'bytes', digest: digest(body) };
  }
  const type = String(headers['content-type'] ?? '').toLowerCase();
  if (type.includes('json')) {
    try {
      return { kind: 'json', value: JSON.parse(bytes.toString('utf8')) };
    } catch {
      // Not valid JSON after all: compared as text below.
    }
  }
  if (type.includes('json') || type.startsWith('text/') || type.includes('xml') || type.includes('javascript')) return { kind: 'text', value: bytes.toString('utf8') };
  return { kind: 'bytes', digest: digest(bytes) };
}

/** A value as one short line, for showing a difference. */
export function preview(value) {
  const text = value === undefined ? 'undefined' : (JSON.stringify(value) ?? String(value));
  return text.length > 120 ? `${text.slice(0, 117)}...` : text;
}

const kindOf = (value) => (value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value);

/** Where two values differ, as paths such as $.items[2].price; at most `limit` of them. */
export function diffValues(a, b, path = '$', out = [], limit = 20) {
  if (out.length >= limit || Object.is(a, b)) return out;
  const kind = kindOf(a);
  if (kind !== kindOf(b) || (kind !== 'object' && kind !== 'array')) {
    out.push({ path, old: preview(a), new: preview(b) });
    return out;
  }
  if (kind === 'array') {
    if (a.length !== b.length) out.push({ path: `${path}.length`, old: String(a.length), new: String(b.length) });
    for (let i = 0; i < Math.min(a.length, b.length) && out.length < limit; i++) diffValues(a[i], b[i], `${path}[${i}]`, out, limit);
    return out;
  }
  const names = [...new Set([...Object.keys(a), ...Object.keys(b)])].sort();
  for (const name of names) {
    if (out.length >= limit) break;
    const child = /^[A-Za-z_$][\w$]*$/.test(name) ? `${path}.${name}` : `${path}[${JSON.stringify(name)}]`;
    if (!Object.hasOwn(b, name)) out.push({ path: child, old: preview(a[name]), new: 'missing' });
    else if (!Object.hasOwn(a, name)) out.push({ path: child, old: 'missing', new: preview(b[name]) });
    else diffValues(a[name], b[name], child, out, limit);
  }
  return out;
}
```

- [ ] **Step 5: Run it and see it pass, on Windows and on Linux**

Run: `node --test --test-concurrency=1 twin/test/serialize.test.js`, then in WSL `sh twin/scripts/test-linux.sh test/serialize.test.js`

Expected: `# pass 5`, `# fail 0` on both.

- [ ] **Step 6: Commit**

```bash
git add twin
git commit -m "Twin: name calls and read answers by value"
```

---

### Task 2: Comparing two runs

**Files:**
- Create: `twin/src/compare.js`
- Test: `twin/test/compare.test.js`

- [ ] **Step 1: Write the failing test**

`twin/test/compare.test.js`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compareRuns, faithful } from '../src/compare.js';

const run = (status, value, more = {}) => ({
  status,
  headers: { 'content-type': 'application/json', date: more.date ?? 'Mon, 01 Jan 2024 12:00:00 GMT' },
  body: Buffer.from(JSON.stringify(value)),
  effects: more.effects ?? [],
  events: more.events ?? [],
  error: more.error,
  timedOut: more.timedOut,
});

test('the same answer is the same, whatever the date header says', () => {
  assert.deepEqual(compareRuns(run(200, { a: 1 }), run(200, { a: 1 }, { date: 'Tue, 02 Jan 2024 12:00:00 GMT' })), { outcome: 'same', kinds: [], details: [] });
});

test('a server error or a thrown error where the old code answered is a crash; no answer at all is a hang', () => {
  assert.ok(compareRuns(run(200, { ok: true }), run(500, { error: 'x' })).kinds.includes('crash'));
  const thrown = compareRuns(run(200, {}), run(0, null, { error: 'uncaught TypeError: x' }));
  assert.equal(thrown.kinds[0], 'crash');
  assert.deepEqual(thrown.details[0], { path: 'result', old: 'status 200', new: 'uncaught TypeError: x' });
  assert.equal(compareRuns(run(200, {}), run(0, null, { error: 'no answer within 500 ms', timedOut: true })).kinds[0], 'hang');
});

test('a refused request that now succeeds is a door that opened', () => {
  assert.ok(compareRuns(run(401, { error: 'sign in first' }), run(200, { users: 3 })).kinds.includes('door-opened'));
});

test('a changed field is named by its path', () => {
  const verdict = compareRuns(run(200, { currency: 'JPY', total: 4051 }), run(200, { currency: 'JPY', total: 4050.9 }));
  assert.deepEqual(verdict, { outcome: 'different', kinds: ['body'], details: [{ path: '$.total', old: '4051', new: '4050.9' }] });
});

test('an extra write is a difference in effects', () => {
  const once = ['POST https://pay.example/charge abc'];
  assert.deepEqual(compareRuns(run(200, {}, { effects: once }), run(200, {}, { effects: [...once, ...once] })).kinds, ['effects']);
});

test('a call a ghost may not make leaves the request not compared', () => {
  const verdict = compareRuns(run(200, {}), run(200, {}, { events: [{ type: 'denied', detail: 'a network connection to db.example:5432' }] }));
  assert.equal(verdict.outcome, 'not-compared');
});

test('the old code\'s ghost must reproduce the real status and writes, not the real clock', () => {
  assert.equal(faithful(run(200, { at: 1 }), run(200, { at: 2 })), null);
  assert.match(faithful(run(200, {}), run(503, {})), /answered 503/);
  assert.match(faithful(run(200, {}, { effects: ['POST https://x.example/ -'] }), run(200, {})), /wrote other things/);
});
```

- [ ] **Step 2: Run it and see it fail**

Run: `node --test --test-concurrency=1 twin/test/compare.test.js`

Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `twin/src/compare.js`.

- [ ] **Step 3: Write the code**

`twin/src/compare.js`:

```js
import { diffValues, readBody } from './serialize.js';

/** Headers that change on every answer and say nothing about behaviour. */
export const VOLATILE_HEADERS = new Set(['date', 'etag', 'last-modified', 'age', 'expires', 'content-length', 'connection', 'keep-alive', 'transfer-encoding', 'x-request-id', 'x-response-time', 'server-timing', 'set-cookie']);

const refused = (status) => status === 401 || status === 403 || status === 404 || status === 405 || (status >= 300 && status < 400);
const succeeded = (status) => status >= 200 && status < 300;
const writes = (list) => [...(list ?? [])].sort().join('\n');
const strayCalls = (run) => run.events.filter((e) => e.type === 'unmatched-read' || e.type === 'unmatched-write');

/**
 * Compares two runs of one request: `a` from the old code, `b` from the new.
 * A run is { status, headers, body: Buffer, effects: string[], events: [{ type, detail }], error?, timedOut? }.
 * Answers { outcome: 'same' | 'different' | 'not-compared', kinds, details: [{ path, old, new }] }.
 */
export function compareRuns(a, b) {
  const denied = [...a.events, ...b.events].filter((e) => e.type === 'denied');
  if (denied.length) return { outcome: 'not-compared', kinds: ['unsupported'], details: denied.slice(0, 5).map((e) => ({ path: 'io', old: '', new: e.detail })) };
  const kinds = new Set();
  const details = [];
  const add = (kind, path, old, now) => {
    kinds.add(kind);
    if (details.length < 20) details.push({ path, old, new: now });
  };
  if (a.error || b.error) {
    if (!a.error) add(b.timedOut ? 'hang' : 'crash', 'result', `status ${a.status}`, b.error);
    else if (!b.error) add('fixed', 'result', a.error, `status ${b.status}`);
    else if (a.error !== b.error) add('changed-error', 'result', a.error, b.error);
  } else {
    if (a.status !== b.status) {
      const kind = b.status >= 500 && a.status < 500 ? 'crash' : refused(a.status) && succeeded(b.status) ? 'door-opened' : 'status';
      add(kind, 'status', String(a.status), String(b.status));
    }
    const names = [...new Set([...Object.keys(a.headers), ...Object.keys(b.headers)])].filter((h) => !VOLATILE_HEADERS.has(h)).sort();
    for (const name of names) {
      const x = a.headers[name] === undefined ? 'missing' : String(a.headers[name]);
      const y = b.headers[name] === undefined ? 'missing' : String(b.headers[name]);
      if (x !== y) add('headers', `header ${name}`, x, y);
    }
    const x = readBody(a.body, a.headers);
    const y = readBody(b.body, b.headers);
    if (x.kind !== y.kind) add('body', 'body', x.kind, y.kind);
    else if (x.kind === 'bytes') {
      if (x.digest !== y.digest) add('body', 'body', `sha256 ${x.digest.slice(0, 16)}`, `sha256 ${y.digest.slice(0, 16)}`);
    } else {
      for (const d of diffValues(x.value, y.value)) add('body', d.path, d.old, d.new);
    }
  }
  const before = writes(a.effects);
  const after = writes(b.effects);
  if (before !== after) add('effects', 'writes', before || 'none', after || 'none');
  if (strayCalls(b).length > strayCalls(a).length) add('new-calls', 'calls', strayCalls(a).map((e) => e.detail).join(', ') || 'none', strayCalls(b).map((e) => e.detail).join(', '));
  return kinds.size ? { outcome: 'different', kinds: [...kinds], details } : { outcome: 'same', kinds: [], details: [] };
}

/**
 * Whether the old code's ghost reproduced the real run: the same status and the same writes, nothing refused, and no call the real
 * run did not make. Bodies may differ (real time, real randomness); the two ghosts are compared with each other for that.
 * Answers null when faithful, or the reason it is not.
 */
export function faithful(real, ghost) {
  if (ghost.error) return `the old code's ghost failed: ${ghost.error}`;
  const denied = ghost.events.find((e) => e.type === 'denied');
  if (denied) return `the ghost needed something it may not use: ${denied.detail}`;
  const stray = strayCalls(ghost);
  if (stray.length) return `the old code's ghost made a call the real run did not: ${stray[0].detail}`;
  if (real.status !== ghost.status) return `the old code's ghost answered ${ghost.status}, the real run ${real.status}`;
  if (writes(real.effects) !== writes(ghost.effects)) return 'the old code\'s ghost wrote other things than the real run';
  return null;
}
```

- [ ] **Step 4: Run it and see it pass, on Windows and on Linux**

Run: `node --test --test-concurrency=1 twin/test/compare.test.js`, then `sh twin/scripts/test-linux.sh test/compare.test.js`

Expected: `# pass 7`, `# fail 0`.

- [ ] **Step 5: Commit**

```bash
git add twin
git commit -m "Twin: compare two runs and check the old ghost against reality"
```

---

### Task 3: The ghost's clock and random numbers

**Files:**
- Create: `twin/src/world.js`
- Test: `twin/test/world.test.js`

- [ ] **Step 1: Write the failing test**

The world patches globals, so the test runs it in a separate Node process.

`twin/test/world.test.js`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';

const script = `
import { installWorld, resetWorld } from ${JSON.stringify(new URL('../src/world.js', import.meta.url).href)};
import crypto from 'node:crypto';
installWorld();
const sample = (seed) => {
  resetWorld(1760000000000, seed);
  return [Date.now(), new Date().toISOString(), Date(), Math.random(), crypto.randomUUID(), globalThis.crypto.randomUUID(),
    crypto.randomBytes(4).toString('hex'), performance.now(), new Date(0).getTime(), new Date() instanceof Date];
};
console.log(JSON.stringify([sample(42), sample(42), sample(43)]));
`;

test('a ghost sees a fixed clock and random numbers that repeat for the same seed', () => {
  const [a, b, c] = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8' }));
  assert.deepEqual(a, b);
  assert.equal(a[0], 1760000000000);
  assert.equal(a[1], '2025-10-09T08:53:20.000Z');
  assert.equal(a[7], 0);
  assert.equal(a[8], 0);
  assert.equal(a[9], true);
  assert.notEqual(a[3], c[3]);
  assert.notEqual(a[4], c[4]);
});
```

- [ ] **Step 2: Run it and see it fail**

Run: `node --test --test-concurrency=1 twin/test/world.test.js`

Expected: FAIL. The child process exits with `ERR_MODULE_NOT_FOUND` for `world.js`.

- [ ] **Step 3: Write the code** (ported from `src/probe.cjs` `steady()`, but resettable per request)

`twin/src/world.js`:

```js
import crypto from 'node:crypto';
import { syncBuiltinESMExports } from 'node:module';

// Time and chance inside a ghost. Both ghosts of one request see the same clock and the same random numbers, so time and
// randomness can never show up as a difference. The clock does not move during a request.
let now = 0;
let state = 1;

function random() {
  state = (state + 0x6d2b79f5) >>> 0;
  let t = state;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

/** Sets the clock to the moment the real request arrived, and seeds the random numbers for this request. */
export function resetWorld(time, seed) {
  now = time;
  state = seed >>> 0;
}

/** Replaces the clock and every source of randomness in this thread. Call once, before the app loads. */
export function installWorld() {
  const Real = Date;
  function GhostDate(...args) {
    if (!new.target) return new Real(now).toString();
    return Reflect.construct(Real, args.length ? args : [now], new.target);
  }
  GhostDate.prototype = Real.prototype;
  GhostDate.now = () => now;
  GhostDate.UTC = Real.UTC;
  GhostDate.parse = Real.parse;
  globalThis.Date = GhostDate;
  Math.random = random;
  const bytes = (n) => {
    const b = Buffer.alloc(n);
    for (let i = 0; i < n; i++) b[i] = Math.floor(random() * 256);
    return b;
  };
  const fill = (view) => {
    new Uint8Array(view.buffer, view.byteOffset, view.byteLength).set(bytes(view.byteLength));
    return view;
  };
  const uuid = () => {
    const b = bytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    const h = b.toString('hex');
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
  };
  // Some of these are getters on Node's own objects: define them over, never assign.
  const put = (owner, name, value) => {
    try {
      Object.defineProperty(owner, name, { value, writable: true, configurable: true, enumerable: true });
    } catch {
      // A frozen object keeps its own; the old and the new ghost then still behave alike.
    }
  };
  put(crypto, 'randomBytes', (n, callback) => {
    const b = bytes(n);
    if (typeof callback === 'function') {
      process.nextTick(callback, null, b);
      return undefined;
    }
    return b;
  });
  put(crypto, 'randomUUID', uuid);
  put(crypto, 'getRandomValues', fill);
  put(crypto, 'randomInt', (a, b, callback) => {
    const [low, high] = b === undefined || typeof b === 'function' ? [0, a] : [a, b];
    const value = low + Math.floor(random() * (high - low));
    const done = typeof b === 'function' ? b : callback;
    if (typeof done === 'function') {
      process.nextTick(done, null, value);
      return undefined;
    }
    return value;
  });
  if (globalThis.crypto) {
    put(globalThis.crypto, 'randomUUID', uuid);
    put(globalThis.crypto, 'getRandomValues', fill);
  }
  put(performance, 'now', () => 0);
  const hrtime = (previous) => {
    const ns = BigInt(now) * 1000000n;
    const t = [Number(ns / 1000000000n), Number(ns % 1000000000n)];
    if (!previous) return t;
    let seconds = t[0] - previous[0];
    let nanos = t[1] - previous[1];
    if (nanos < 0) {
      seconds -= 1;
      nanos += 1e9;
    }
    return [seconds, nanos];
  };
  hrtime.bigint = () => BigInt(now) * 1000000n;
  put(process, 'hrtime', hrtime);
  syncBuiltinESMExports();
}
```

- [ ] **Step 4: Run it and see it pass, on Windows and on Linux**

Run: `node --test --test-concurrency=1 twin/test/world.test.js`, then `sh twin/scripts/test-linux.sh test/world.test.js`

Expected: `# pass 1`, `# fail 0`.

- [ ] **Step 5: Commit**

```bash
git add twin
git commit -m "Twin: a fixed clock and seeded randomness for ghosts"
```

---

### Task 4: What a ghost may do

**Files:**
- Create: `twin/src/ghost-io.js`
- Test: `twin/test/ghost-io.test.js`

- [ ] **Step 1: Write the failing test**

This one also runs in a separate Node process, because it patches globals.

`twin/test/ghost-io.test.js`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';

const source = (file) => JSON.stringify(new URL(`../src/${file}`, import.meta.url).href);
const script = `
import { installGhostIO, beginJob, endJob } from ${source('ghost-io.js')};
import { callKey } from ${source('keys.js')};
import net from 'node:net';
import fs from 'node:fs';
import { execSync } from 'node:child_process';
installGhostIO({ harnessPort: 1 });
const body = Buffer.from(JSON.stringify({ amount: 5 }));
const file = '/tmp/kern-ghost-' + process.pid;
beginJob([
  { key: callKey('GET', 'https://api.example/rate', null), status: 200, headers: { 'content-type': 'application/json' }, body: Buffer.from('{"rate":2}').toString('base64') },
  { key: callKey('POST', 'https://pay.example/charge', body), status: 200, headers: {}, body: null },
]);
const out = {};
out.rate = await (await fetch('https://api.example/rate')).json();
out.charge = (await fetch('https://pay.example/charge', { method: 'POST', body })).status;
out.second = await fetch('https://pay.example/charge', { method: 'POST', body }).then(() => 'answered', (e) => e.message);
out.unknown = await fetch('https://elsewhere.example/').then(() => 'answered', (e) => e.message);
out.socket = await new Promise((resolve) => net.connect(5432, 'db.example').on('error', (e) => resolve(e.code)));
try { execSync('echo hi'); out.exec = 'ran'; } catch (e) { out.exec = e.code; }
try { fs.writeFileSync(file, 'x'); out.write = 'wrote'; } catch (e) { out.write = e.code; }
out.writeAsync = await fs.promises.writeFile(file, 'x').then(() => 'wrote', (e) => e.code);
out.exists = fs.existsSync(file);
Object.assign(out, endJob());
console.log(JSON.stringify(out));
`;

test('a ghost gets its calls answered from the recording and does nothing real', () => {
  const out = JSON.parse(execFileSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8' }));
  assert.deepEqual(out.rate, { rate: 2 });
  assert.equal(out.charge, 200);
  assert.equal(out.second, 'fetch failed');
  assert.equal(out.unknown, 'fetch failed');
  assert.equal(out.socket, 'EACCES');
  assert.equal(out.exec, 'EACCES');
  assert.equal(out.write, 'EACCES');
  assert.equal(out.writeAsync, 'EACCES');
  assert.equal(out.exists, false);
  assert.equal(out.effects.filter((e) => e.startsWith('POST https://pay.example/charge')).length, 2);
  assert.equal(out.effects.filter((e) => e.startsWith('FS writeFile')).length, 2);
  assert.deepEqual(out.events.map((e) => e.type), ['unmatched-write', 'unmatched-read', 'denied', 'denied']);
});
```

- [ ] **Step 2: Run it and see it fail**

Run: `node --test --test-concurrency=1 twin/test/ghost-io.test.js`

Expected: FAIL. The child process exits with `ERR_MODULE_NOT_FOUND` for `ghost-io.js`.

- [ ] **Step 3: Write the code**

`twin/src/ghost-io.js`:

```js
import net from 'node:net';
import dgram from 'node:dgram';
import fs from 'node:fs';
import childProcess from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
import { callKey, isWrite } from './keys.js';

// What a ghost may do: have its fetch calls answered from the real run's recording, and nothing else. Writes are written down and
// never done; any other way out (sockets, UDP, programs, file writes) is refused, so a ghost can never touch anything real.
let job = null;

/** Starts a request: the real run's calls, queued by name, so each recorded answer is used once. */
export function beginJob(io) {
  const queues = new Map();
  for (const call of io ?? []) {
    if (!queues.has(call.key)) queues.set(call.key, []);
    queues.get(call.key).push(call);
  }
  job = { queues, effects: [], events: [] };
}

/** Ends a request: what the ghost tried to write, and what it was refused or could not find. */
export function endJob() {
  const out = job ? { effects: job.effects, events: job.events } : { effects: [], events: [] };
  job = null;
  return out;
}

function note(type, detail) {
  if (job && job.events.length < 50) job.events.push({ type, detail: String(detail).slice(0, 300) });
}

function wrote(effect) {
  if (job && job.effects.length < 200) job.effects.push(effect);
}

const NULL_BODY = new Set([101, 204, 205, 304]);
const loopback = (host) => ['127.0.0.1', 'localhost', '::1', '[::1]'].includes(String(host));
const refusal = (what) => Object.assign(new Error(`${what} is refused in a Kern ghost run`), { code: 'EACCES' });

/** Seals this thread. `harnessPort` is the loopback port of the ghost's own server: the one connection it may make. */
export function installGhostIO({ harnessPort }) {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async function ghostFetch(input, init) {
    const request = new Request(input, init);
    const url = new URL(request.url);
    if (loopback(url.hostname) && Number(url.port) === harnessPort) return realFetch(request);
    const body = request.body ? Buffer.from(await request.arrayBuffer()) : Buffer.alloc(0);
    const key = callKey(request.method, request.url, body);
    const write = isWrite(request.method);
    if (write) wrote(key);
    const recorded = job?.queues.get(key)?.shift();
    if (!recorded) {
      note(write ? 'unmatched-write' : 'unmatched-read', `${request.method.toUpperCase()} ${request.url}`);
      throw new TypeError('fetch failed');
    }
    if (recorded.error) throw new TypeError('fetch failed');
    const headers = Object.entries(recorded.headers ?? {}).filter(([name]) => !['content-encoding', 'content-length', 'transfer-encoding'].includes(name));
    const answer = NULL_BODY.has(recorded.status) || recorded.body == null ? null : Buffer.from(recorded.body, 'base64');
    return new Response(answer, { status: recorded.status, headers });
  };

  const realConnect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function ghostConnect(...args) {
    const first = Array.isArray(args[0]) ? args[0][0] : args[0];
    const options = first !== null && typeof first === 'object' ? first : { port: first, host: typeof args[1] === 'string' ? args[1] : 'localhost' };
    if (options.path === undefined && loopback(options.host ?? 'localhost') && Number(options.port) === harnessPort) return realConnect.apply(this, args);
    note('denied', `a network connection to ${options.path ?? `${options.host ?? 'localhost'}:${options.port}`}`);
    process.nextTick(() => this.destroy(refusal('A network connection')));
    return this;
  };

  // The app may start its own server when it loads; in a ghost that server never opens. The harness's server is already listening.
  net.Server.prototype.listen = function ghostListen(...args) {
    const done = args.find((a) => typeof a === 'function');
    if (done) process.nextTick(done);
    return this;
  };

  dgram.Socket.prototype.send = function ghostSend(...args) {
    note('denied', 'a UDP message');
    const done = args.find((a) => typeof a === 'function');
    if (done) process.nextTick(done, refusal('A UDP message'));
  };

  for (const name of ['spawn', 'spawnSync', 'exec', 'execSync', 'execFile', 'execFileSync', 'fork']) {
    childProcess[name] = function ghostProgram(file) {
      note('denied', `starting the program ${String(file).trim().split(/\s+/)[0]}`);
      throw refusal('Starting a program');
    };
  }

  for (const name of ['writeFile', 'appendFile', 'unlink', 'rm', 'rmdir', 'rename', 'mkdir', 'copyFile', 'cp', 'truncate', 'symlink', 'link', 'chmod', 'utimes']) {
    const effect = (target) => wrote(`FS ${name} ${String(target)}`);
    fs[`${name}Sync`] = (target) => {
      effect(target);
      throw refusal(`fs.${name}`);
    };
    fs[name] = (target, ...rest) => {
      effect(target);
      const done = rest.findLast((a) => typeof a === 'function');
      if (done) process.nextTick(done, refusal(`fs.${name}`));
    };
    fs.promises[name] = async (target) => {
      effect(target);
      throw refusal(`fs.${name}`);
    };
  }
  fs.createWriteStream = (target) => {
    wrote(`FS createWriteStream ${String(target)}`);
    throw refusal('fs.createWriteStream');
  };
  syncBuiltinESMExports();
}
```

- [ ] **Step 4: Run it and see it pass, on Windows and on Linux**

Run: `node --test --test-concurrency=1 twin/test/ghost-io.test.js`, then `sh twin/scripts/test-linux.sh test/ghost-io.test.js`

Expected: `# pass 1`, `# fail 0`.

- [ ] **Step 5: Commit**

```bash
git add twin
git commit -m "Twin: ghosts replay fetch, write down writes and refuse everything else"
```

---

### Task 5: A ghost pair that answers recorded requests

**Files:**
- Create: `twin/src/worker.js`, `twin/src/coordinator.js`, `twin/test/helpers.js`, `twin/test/apps/http.js`, `twin/test/apps/refactor/old.js`, `twin/test/apps/refactor/new.js`, `twin/test/apps/renamed-field/old.js`, `twin/test/apps/renamed-field/new.js`
- Test: `twin/test/coordinator.test.js`

- [ ] **Step 1: Create the test apps**

`twin/test/apps/http.js`:

```js
// Small helpers the test apps share: plain node:http, like a simple production app.
export async function readJson(req) {
  const parts = [];
  for await (const chunk of req) parts.push(chunk);
  const text = Buffer.concat(parts).toString('utf8');
  return text ? JSON.parse(text) : {};
}

export function send(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) });
  res.end(body);
}
```

`twin/test/apps/refactor/old.js`:

```js
import { send } from '../http.js';

export function createApp() {
  return (req, res) => {
    const n = Number(new URL(req.url, 'http://app').searchParams.get('n') ?? 10);
    let total = 0;
    for (let i = 1; i <= n; i++) total += i;
    send(res, 200, { n, total });
  };
}
```

`twin/test/apps/refactor/new.js`:

```js
import { send } from '../http.js';

// The same answer, written another way.
export function createApp() {
  return (req, res) => {
    const n = Number(new URL(req.url, 'http://app').searchParams.get('n') ?? 10);
    send(res, 200, { n, total: Array.from({ length: n }, (_, i) => i + 1).reduce((a, b) => a + b, 0) });
  };
}
```

`twin/test/apps/renamed-field/old.js`:

```js
import { send } from '../http.js';

export function createApp() {
  return (req, res) => send(res, 200, { user_name: 'ada', plan: 'pro' });
}
```

`twin/test/apps/renamed-field/new.js`:

```js
import { send } from '../http.js';

// The agent renamed user_name to username; older clients still read user_name.
export function createApp() {
  return (req, res) => send(res, 200, { username: 'ada', plan: 'pro' });
}
```

`twin/test/helpers.js` (Task 6 extends it):

```js
import { fileURLToPath } from 'node:url';

/** One version of a test app, as the twin loads it: an absolute file and the function that builds the request listener. */
export const app = (name, version) => ({ module: fileURLToPath(new URL(`./apps/${name}/${version}.js`, import.meta.url)), export: 'createApp' });
```

- [ ] **Step 2: Write the failing test**

`twin/test/coordinator.test.js`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Coordinator } from '../src/coordinator.js';
import { app } from './helpers.js';

const options = (name) => ({ old: app(name, 'old'), candidate: app(name, 'new'), timeoutMs: 3000, startMs: 20000, queueLimit: 10, ghostMemoryMb: 256, maxRestarts: 5, log: null });

/** A request as the real app served it: what came in, what went out, and no outgoing calls. */
function served(url, answer) {
  return {
    start: 1760000000000,
    seed: 7,
    request: { method: 'GET', url, headers: { host: 'shop.example' }, body: '' },
    response: { status: 200, headers: { 'content-type': 'application/json' }, body: Buffer.from(JSON.stringify(answer)) },
    io: [],
    effects: [],
  };
}

test('two ghosts answer a recorded request and the difference is named', async () => {
  const pair = new Coordinator(options('renamed-field'));
  await pair.start();
  assert.equal(pair.disabled, null);
  pair.submit(served('/user', { user_name: 'ada', plan: 'pro' }));
  await pair.idle();
  assert.equal(pair.counts.different, 1);
  const [found] = pair.differences();
  assert.equal(found.url, '/user');
  assert.deepEqual(found.kinds, ['body']);
  assert.deepEqual(found.details, [
    { path: '$.user_name', old: '"ada"', new: 'missing' },
    { path: '$.username', old: 'missing', new: '"ada"' },
  ]);
  await pair.close();
});

test('a change that answers the same is counted as the same', async () => {
  const pair = new Coordinator(options('refactor'));
  await pair.start();
  pair.submit(served('/?n=5', { n: 5, total: 15 }));
  await pair.idle();
  assert.deepEqual([pair.counts.same, pair.counts.different, pair.counts.notCompared], [1, 0, 0]);
  await pair.close();
});
```

- [ ] **Step 3: Run it and see it fail**

Run: `node --test --test-concurrency=1 twin/test/coordinator.test.js`

Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `twin/src/coordinator.js`.

- [ ] **Step 4: Write the ghost**

`twin/src/worker.js`:

```js
import { parentPort, workerData } from 'node:worker_threads';
import http from 'node:http';
import { pathToFileURL } from 'node:url';
import { installWorld, resetWorld } from './world.js';
import { installGhostIO, beginJob, endJob } from './ghost-io.js';

// A ghost: one version of the app in its own thread, answering copies of real requests in a sealed world. Its own server listens on
// loopback before the seal goes on; after that, the app can reach nothing but that server.
const realRequest = http.request;
let handler = null;
const server = http.createServer((req, res) => handler(req, res));
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const port = server.address().port;

installWorld();
installGhostIO({ harnessPort: port });
const app = await import(pathToFileURL(workerData.module).href);
const factory = app[workerData.export];
if (typeof factory !== 'function') throw new Error(`has no function named ${workerData.export}`);
handler = await factory();
if (typeof handler !== 'function') throw new Error(`${workerData.export}() must return a request listener: (req, res) => { ... }`);

let finish = null;
function crashed(error) {
  // The app's state may now be broken: the coordinator restarts both ghosts after this request.
  parentPort.postMessage({ type: 'dirty' });
  const message = `uncaught ${error?.name ?? 'error'}: ${String(error?.message ?? error).slice(0, 300)}`;
  if (finish) {
    const done = finish;
    finish = null;
    done({ error: message });
  }
}
process.on('uncaughtException', crashed);
process.on('unhandledRejection', crashed);

parentPort.on('message', (message) => {
  if (message?.type === 'job') run(message.id, message.job);
});
parentPort.postMessage({ type: 'ready' });

async function run(id, job) {
  resetWorld(job.start, job.seed);
  beginJob(job.io);
  const result = await new Promise((resolve) => {
    finish = resolve;
    const body = Buffer.from(job.request.body ?? '', 'base64');
    const headers = { ...job.request.headers, 'content-length': String(body.length) };
    const req = realRequest({ host: '127.0.0.1', port, method: job.request.method, path: job.request.url, headers, agent: false }, (res) => {
      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) }));
      res.on('error', (error) => resolve({ error: `the answer broke off: ${error.message}` }));
    });
    req.on('error', (error) => resolve({ error: `no answer: ${error.message}` }));
    req.end(body);
  });
  finish = null;
  const io = endJob();
  parentPort.postMessage({
    type: 'result', id,
    result: { status: result.status, headers: result.headers, error: result.error, body: result.body ? result.body.toString('base64') : null, effects: io.effects, events: io.events },
  });
}
```

- [ ] **Step 5: Write the coordinator**

`twin/src/coordinator.js`:

```js
import { Worker } from 'node:worker_threads';
import { appendFile } from 'node:fs/promises';
import { compareRuns, faithful } from './compare.js';

const GHOST = new URL('./worker.js', import.meta.url);
const KEEP = 1000;

/** Starts one ghost and waits until its version of the app has loaded. */
function spawn(side, { startMs, memoryMb }) {
  return new Promise((resolve, reject) => {
    const worker = new Worker(GHOST, { workerData: side, stdout: true, stderr: true, resourceLimits: { maxOldGenerationSizeMb: memoryMb } });
    // A ghost's console output is not the app's: it is read and dropped, never mixed into production logs.
    worker.stdout.resume();
    worker.stderr.resume();
    let timer = null;
    const cleanup = () => {
      clearTimeout(timer);
      worker.off('message', onMessage);
      worker.off('error', onError);
      worker.off('exit', onExit);
    };
    const fail = (error) => {
      cleanup();
      worker.terminate();
      reject(error);
    };
    const onMessage = (message) => {
      if (message?.type === 'ready') {
        cleanup();
        // From here on an error (running out of memory, say) ends only this ghost. Without a listener, Node would throw the
        // worker's 'error' event in the app's own thread and end the app: one must always be attached.
        worker.on('error', () => {});
        resolve(worker);
      }
    };
    const onError = (error) => fail(error);
    const onExit = (code) => fail(new Error(`stopped while loading (exit ${code})`));
    timer = setTimeout(() => fail(new Error(`did not load within ${startMs} ms`)), startMs);
    worker.on('message', onMessage);
    worker.on('error', onError);
    worker.on('exit', onExit);
  });
}

/** Sends one request to one ghost and answers its result, or why there is none. `dirty` means the ghost must be restarted. */
function ask(worker, id, job, timeoutMs) {
  return new Promise((resolve) => {
    let dirty = false;
    let timer = null;
    const done = (result) => {
      clearTimeout(timer);
      worker.off('message', onMessage);
      worker.off('exit', onExit);
      resolve({ ...result, dirty: dirty || Boolean(result.timedOut || result.exited) });
    };
    const onMessage = (message) => {
      if (message?.type === 'dirty') dirty = true;
      else if (message?.type === 'result' && message.id === id) done(message.result);
    };
    const onExit = (code) => done({ error: `the ghost stopped (exit ${code})`, exited: true });
    timer = setTimeout(() => done({ error: `no answer within ${timeoutMs} ms`, timedOut: true }), timeoutMs);
    worker.on('message', onMessage);
    worker.on('exit', onExit);
    worker.postMessage({ type: 'job', id, job });
  });
}

function toRun(result) {
  return {
    status: result.status,
    headers: result.headers ?? {},
    body: result.body ? Buffer.from(result.body, 'base64') : Buffer.alloc(0),
    effects: result.effects ?? [],
    events: result.events ?? [],
    error: result.error,
    timedOut: result.timedOut,
  };
}

/**
 * Runs the ghost pair: the old and the new version, each in its own thread. They answer the same requests in the same order and
 * always restart together, so their histories stay identical and a difference can only come from the code.
 */
export class Coordinator {
  #options;
  #queue = [];
  #busy = false;
  #seq = 0;
  #pair = null;
  #dirty = false;
  #waiters = [];
  #found = [];
  disabled = null;
  counts = { sampled: 0, skipped: 0, dropped: 0, same: 0, different: 0, notCompared: 0, restarts: 0, logErrors: 0 };
  reasons = {};

  constructor(options) {
    this.#options = options;
  }

  async start() {
    try {
      this.#pair = await this.#spawnPair();
    } catch (error) {
      await this.disable(`the ghosts could not start: ${error.message}`);
    }
  }

  async #spawnPair() {
    const settings = { startMs: this.#options.startMs, memoryMb: this.#options.ghostMemoryMb };
    const named = (label, side) => spawn(side, settings).catch((error) => {
      throw new Error(`the ${label} version (${side.module}) ${error.message}`);
    });
    const [old, candidate] = await Promise.allSettled([named('old', this.#options.old), named('new', this.#options.candidate)]);
    if (old.status === 'rejected' || candidate.status === 'rejected') {
      await Promise.allSettled([old, candidate].filter((s) => s.status === 'fulfilled').map((s) => s.value.terminate()));
      throw old.status === 'rejected' ? old.reason : candidate.reason;
    }
    this.#dirty = false;
    for (const worker of [old.value, candidate.value]) {
      worker.on('message', (message) => {
        if (message?.type === 'dirty') this.#dirty = true;
      });
      worker.on('error', () => {
        this.#dirty = true;
      });
    }
    return { old: old.value, candidate: candidate.value };
  }

  async #restart() {
    this.counts.restarts++;
    const pair = this.#pair;
    this.#pair = null;
    if (pair) await Promise.allSettled([pair.old.terminate(), pair.candidate.terminate()]);
    if (this.disabled) return;
    if (this.counts.restarts > this.#options.maxRestarts) {
      await this.disable(`the ghosts had to restart more than ${this.#options.maxRestarts} times`);
      return;
    }
    try {
      this.#pair = await this.#spawnPair();
    } catch (error) {
      await this.disable(`the ghosts could not restart: ${error.message}`);
    }
  }

  /** Whether a request may be recorded now. A full queue sheds load: the customer is served as always, the request is just not compared. */
  admit() {
    if (this.disabled || !this.#pair) return false;
    if (this.#queue.length >= this.#options.queueLimit) {
      this.counts.dropped++;
      return false;
    }
    return true;
  }

  /** Takes the copy of one served request; requests are compared one at a time, in order. */
  submit(capture) {
    if (this.disabled) return;
    if (capture.skipped) {
      this.counts.skipped++;
      this.#reason(`skipped: ${capture.skipped}`);
      return;
    }
    if (this.#queue.length >= this.#options.queueLimit) {
      this.counts.dropped++;
      return;
    }
    this.counts.sampled++;
    this.#queue.push(capture);
    this.#pump();
  }

  async #pump() {
    if (this.#busy) return;
    this.#busy = true;
    try {
      while (this.#queue.length && !this.disabled) {
        const capture = this.#queue.shift();
        try {
          await this.#compare(capture);
        } catch (error) {
          await this.disable(`the twin failed inside: ${error.message}`);
        }
      }
    } finally {
      this.#busy = false;
      this.#settle();
    }
  }

  async #compare(capture) {
    if (!this.#pair) return;
    const id = ++this.#seq;
    const job = { start: capture.start, seed: capture.seed, request: capture.request, io: capture.io };
    const [old, candidate] = await Promise.all([ask(this.#pair.old, id, job, this.#options.timeoutMs), ask(this.#pair.candidate, id, job, this.#options.timeoutMs)]);
    if (this.disabled) return;
    if (old.dirty || candidate.dirty || this.#dirty) await this.#restart();
    const real = { status: capture.response.status, headers: capture.response.headers, body: capture.response.body, effects: capture.effects, events: [] };
    const oldRun = toRun(old);
    const unfaithful = faithful(real, oldRun);
    const verdict = unfaithful
      ? { outcome: 'not-compared', kinds: ['unfaithful'], details: [{ path: 'ghost', old: '', new: unfaithful }] }
      : compareRuns(oldRun, toRun(candidate));
    if (verdict.outcome === 'same') this.counts.same++;
    else if (verdict.outcome === 'different') this.counts.different++;
    else {
      this.counts.notCompared++;
      this.#reason(`not compared: ${verdict.details[0]?.new ?? verdict.kinds[0]}`);
    }
    if (verdict.outcome === 'different') await this.#remember({ at: new Date(capture.start).toISOString(), method: capture.request.method, url: capture.request.url, ...verdict });
  }

  #reason(text) {
    const key = text.slice(0, 160);
    if (key in this.reasons || Object.keys(this.reasons).length < 50) this.reasons[key] = (this.reasons[key] ?? 0) + 1;
  }

  async #remember(entry) {
    if (this.#found.length < KEEP) this.#found.push(entry);
    if (!this.#options.log) return;
    try {
      await appendFile(this.#options.log, `${JSON.stringify(entry)}\n`);
    } catch {
      this.counts.logErrors++;
    }
  }

  /** Resolves once every submitted request has been compared. */
  idle() {
    if (!this.#busy && !this.#queue.length) return Promise.resolve();
    return new Promise((resolve) => this.#waiters.push(resolve));
  }

  #settle() {
    if (this.#busy || this.#queue.length) return;
    for (const resolve of this.#waiters.splice(0)) resolve();
  }

  /** Switches the twin off for good, keeping the first reason. The app is never affected. */
  disable(reason) {
    if (!this.disabled) this.disabled = reason;
    this.#queue.length = 0;
    const pair = this.#pair;
    this.#pair = null;
    this.#settle();
    return pair ? Promise.allSettled([pair.old.terminate(), pair.candidate.terminate()]) : Promise.resolve();
  }

  close() {
    return this.disable('closed');
  }

  snapshot() {
    return { ...this.counts, queued: this.#queue.length, disabled: this.disabled, reasons: { ...this.reasons } };
  }

  differences() {
    return this.#found.slice();
  }
}
```

- [ ] **Step 6: Run it and see it pass, on Windows and on Linux**

Run: `node --test --test-concurrency=1 twin/test/coordinator.test.js`, then `sh twin/scripts/test-linux.sh test/coordinator.test.js`

Expected: `# pass 2`, `# fail 0`.

- [ ] **Step 7: Commit**

```bash
git add twin
git commit -m "Twin: a ghost pair that answers recorded requests in a sealed world"
```

---

### Task 6: Watching the real app

**Files:**
- Create: `twin/src/capture.js`, `twin/src/record.js`, `twin/src/index.js`
- Modify: `twin/test/helpers.js` (replace the whole file)
- Test: `twin/test/scenarios.test.js`

- [ ] **Step 1: Extend the helpers**

Replace `twin/test/helpers.js` with:

```js
import http from 'node:http';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../src/index.js';

/** One version of a test app, as the twin loads it: an absolute file and the function that builds the request listener. */
export const app = (name, version) => ({ module: fileURLToPath(new URL(`./apps/${name}/${version}.js`, import.meta.url)), export: 'createApp' });

/** A local HTTP server for a listener. */
export async function listen(listener) {
  const server = http.createServer(listener);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    close: () => new Promise((resolve) => {
      server.closeAllConnections();
      server.close(resolve);
    }),
  };
}

/** Production as it would run: the old app serves customers, and the twin runs the old and the new version as ghosts. */
export async function production(name, options = {}) {
  const twin = await createTwin({ old: app(name, 'old'), candidate: app(name, 'new'), timeoutMs: 3000, ...options });
  const { createApp } = await import(new URL(`./apps/${name}/old.js`, import.meta.url).href);
  const server = await listen(twin.wrap(await createApp()));
  return {
    twin,
    url: server.url,
    close: async () => {
      await server.close();
      await twin.close();
    },
  };
}

export const post = (url, value) => fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(value) });
```

- [ ] **Step 2: Write the failing test**

`twin/test/scenarios.test.js`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { production } from './helpers.js';

test('a change that behaves the same is reported as the same', async () => {
  const shop = await production('refactor');
  assert.deepEqual(await (await fetch(`${shop.url}/?n=5`)).json(), { n: 5, total: 15 });
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.deepEqual([stats.same, stats.different, stats.notCompared, stats.skipped], [1, 0, 0, 0], JSON.stringify(stats));
  await shop.close();
});

test('a renamed field is caught while the customer gets the old answer', async () => {
  const shop = await production('renamed-field');
  assert.deepEqual(await (await fetch(`${shop.url}/user`)).json(), { user_name: 'ada', plan: 'pro' });
  await shop.twin.idle();
  const [found] = shop.twin.differences();
  assert.equal(found.method, 'GET');
  assert.equal(found.url, '/user');
  assert.ok(found.details.some((d) => d.path === '$.username' && d.old === 'missing'), JSON.stringify(found));
  await shop.close();
});
```

- [ ] **Step 3: Run it and see it fail**

Run: `node --test --test-concurrency=1 twin/test/scenarios.test.js`

Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `twin/src/index.js`.

- [ ] **Step 4: Write the code**

`twin/src/capture.js`:

```js
import { AsyncLocalStorage } from 'node:async_hooks';
import { randomInt } from 'node:crypto';

/** The record of the sampled request being served, so the calls the app makes while serving it are written down with it. */
export const current = new AsyncLocalStorage();

// Headers about the connection, not the request; the ghost's own connection sets its own.
const HOP = ['connection', 'keep-alive', 'proxy-connection', 'transfer-encoding', 'te', 'trailer', 'upgrade', 'content-length'];

function lower(headers) {
  const out = {};
  for (const [name, value] of Object.entries(headers ?? {})) out[name.toLowerCase()] = Array.isArray(value) ? value.map(String) : String(value);
  return out;
}

function bytesOf(chunk, encoding) {
  if (Buffer.isBuffer(chunk)) return chunk;
  if (typeof chunk === 'string') return Buffer.from(chunk, typeof encoding === 'string' ? encoding : 'utf8');
  if (chunk instanceof Uint8Array) return Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  return null;
}

/**
 * Wraps a Node request listener. A request that is not sampled goes straight to the listener. A sampled request is served exactly
 * as before while Kern keeps a copy of what came in, what went out and every outgoing call; `deliver` gets a promise of that copy
 * once the answer has gone out.
 */
export function wrapListener(listener, { sample, maxBodyBytes, deliver, onError }) {
  return function kernTwin(req, res) {
    let record = null;
    try {
      if (sample()) record = begin(req, res, maxBodyBytes, deliver);
    } catch (error) {
      onError(error);
      record = null;
    }
    if (!record) return listener(req, res);
    return current.run(record, () => listener(req, res));
  };
}

function begin(req, res, maxBodyBytes, deliver) {
  const record = {
    start: Date.now(), seed: randomInt(2 ** 31), io: [], pending: new Set(),
    incoming: [], outgoing: [], sizes: { incoming: 0, outgoing: 0 }, head: null, problem: null, done: false,
  };
  const copy = (side, chunk, encoding) => {
    try {
      if (record.problem || chunk == null) return;
      const bytes = bytesOf(chunk, encoding);
      if (!bytes) return;
      record.sizes[side] += bytes.length;
      if (record.sizes[side] > maxBodyBytes) record.problem = `a body over ${maxBodyBytes} bytes`;
      else record[side].push(Buffer.from(bytes));
    } catch {
      record.problem ??= 'a body Kern could not copy';
    }
  };
  // The HTTP parser hands every piece of the body to push(), however the app reads it (events, read() or async iteration).
  const push = req.push;
  req.push = function (chunk, encoding) {
    copy('incoming', chunk, encoding);
    return push.call(this, chunk, encoding);
  };
  const { writeHead, write, end } = res;
  res.writeHead = function (...args) {
    try {
      const headers = args.slice(1).find((a) => a && typeof a === 'object' && !Array.isArray(a));
      if (headers) record.head = headers;
    } catch {
      // The answer goes out unchanged either way.
    }
    return writeHead.apply(this, args);
  };
  res.write = function (chunk, encoding, callback) {
    copy('outgoing', chunk, encoding);
    return write.call(this, chunk, encoding, callback);
  };
  res.end = function (chunk, encoding, callback) {
    if (typeof chunk !== 'function') copy('outgoing', chunk, encoding);
    return end.call(this, chunk, encoding, callback);
  };
  res.once('finish', () => deliver(finish(req, res, record)));
  return record;
}

async function finish(req, res, record) {
  // Calls the app starts right after answering still belong to this request: give them, and copies still being read, a moment.
  await new Promise((resolve) => setImmediate(resolve));
  if (record.pending.size) await Promise.race([Promise.allSettled([...record.pending]), new Promise((resolve) => setTimeout(resolve, 100))]);
  record.done = true;
  if (record.problem) return { skipped: record.problem };
  if (record.pending.size) return { skipped: 'an outgoing call was still running' };
  const headers = lower(req.headers);
  for (const name of HOP) delete headers[name];
  return {
    start: record.start,
    seed: record.seed,
    request: { method: req.method, url: req.url, headers, body: Buffer.concat(record.incoming).toString('base64') },
    response: { status: res.statusCode, headers: { ...lower(res.getHeaders()), ...lower(record.head) }, body: Buffer.concat(record.outgoing) },
    io: record.io,
    effects: record.io.filter((call) => call.write).map((call) => call.key),
  };
}
```

`twin/src/record.js`:

```js
import { current } from './capture.js';
import { callKey, isWrite } from './keys.js';

let installed = false;

/** Reads a stream to its end; null when it is larger than `cap` bytes (the rest is cancelled). */
export async function readCapped(stream, cap) {
  if (!stream) return Buffer.alloc(0);
  const reader = stream.getReader();
  const parts = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return Buffer.concat(parts);
    size += value.byteLength;
    if (size > cap) {
      await reader.cancel().catch(() => {});
      return null;
    }
    parts.push(Buffer.from(value));
  }
}

/** Writes down every fetch a sampled request makes: what was asked and what came back. Calls outside sampled requests pass straight through. */
export function recordFetch(maxBodyBytes) {
  if (installed) return;
  installed = true;
  const realFetch = globalThis.fetch;
  globalThis.fetch = function kernFetch(input, init) {
    const record = current.getStore();
    if (!record || record.done) return realFetch(input, init);
    return recorded(record, realFetch, input, init, maxBodyBytes);
  };
}

async function recorded(record, realFetch, input, init, maxBodyBytes) {
  // The copy is complete only once the answer has been read to the end; finish() waits for this, a little.
  let settle;
  const tracked = new Promise((resolve) => {
    settle = resolve;
  });
  record.pending.add(tracked);
  tracked.then(() => record.pending.delete(tracked));
  let request;
  try {
    request = new Request(input, init);
  } catch {
    settle();
    return realFetch(input, init);
  }
  let body;
  try {
    body = request.body ? await readCapped(request.clone().body, maxBodyBytes) : Buffer.alloc(0);
  } catch {
    body = null;
  }
  if (body === null) {
    record.problem ??= 'an outgoing call had a body Kern could not copy';
    settle();
    return realFetch(request);
  }
  const method = request.method.toUpperCase();
  const call = { key: callKey(method, request.url, body), method, url: request.url, write: isWrite(method), status: 0, headers: {}, body: null, error: null };
  record.io.push(call);
  let response;
  try {
    response = await realFetch(request);
  } catch (error) {
    // A call that failed for real fails the same way in both ghosts.
    call.error = String(error?.message ?? error);
    settle();
    throw error;
  }
  call.status = response.status;
  call.headers = Object.fromEntries(response.headers);
  readCapped(response.clone().body, maxBodyBytes).then(
    (copy) => {
      if (copy === null) record.problem ??= 'an outgoing call answered with a body over the limit';
      else call.body = copy.toString('base64');
    },
    () => {
      record.problem ??= 'an outgoing call\'s answer could not be copied';
    },
  ).finally(settle);
  return response;
}
```

`twin/src/index.js`:

```js
import { wrapListener } from './capture.js';
import { recordFetch } from './record.js';
import { Coordinator } from './coordinator.js';

function version(value, name) {
  if (!value || typeof value.module !== 'string' || typeof value.export !== 'string') throw new TypeError(`${name} must be { module: '/absolute/path/to/app.js', export: 'createApp' }`);
  return { module: value.module, export: value.export };
}

function number(value, fallback, min, max, name) {
  const n = value ?? fallback;
  if (typeof n !== 'number' || !(n >= min && n <= max)) throw new TypeError(`${name} must be a number from ${min} to ${max}`);
  return n;
}

/**
 * Starts a twin: the old and the new version of the app run as ghosts and answer copies of sampled real requests.
 * The twin never changes what a customer gets; if anything goes wrong inside it, it switches itself off and says why.
 */
export async function createTwin(options = {}) {
  const settings = {
    old: version(options.old, 'old'),
    candidate: version(options.candidate, 'candidate'),
    sampleRate: number(options.sampleRate, 1, 0, 1, 'sampleRate'),
    timeoutMs: number(options.timeoutMs, 5000, 100, 60000, 'timeoutMs'),
    startMs: number(options.startMs, 20000, 1000, 120000, 'startMs'),
    queueLimit: number(options.queueLimit, 100, 1, 10000, 'queueLimit'),
    maxBodyBytes: number(options.maxBodyBytes, 1 << 20, 1024, 64 << 20, 'maxBodyBytes'),
    ghostMemoryMb: number(options.ghostMemoryMb, 512, 64, 16384, 'ghostMemoryMb'),
    maxRestarts: number(options.maxRestarts, 20, 0, 1000, 'maxRestarts'),
    log: options.log ?? null,
  };
  const coordinator = new Coordinator(settings);
  if (process.env.KERN_TWIN === 'off') await coordinator.disable('switched off with KERN_TWIN=off');
  else await coordinator.start();
  if (!coordinator.disabled) recordFetch(settings.maxBodyBytes);
  const inflight = new Set();
  const onError = (error) => {
    coordinator.disable(`recording failed: ${error?.message ?? error}`);
  };
  const deliver = (copy) => {
    inflight.add(copy);
    copy.then((capture) => coordinator.submit(capture), onError).finally(() => inflight.delete(copy));
  };
  const sample = () => Math.random() < settings.sampleRate && coordinator.admit();
  return {
    wrap: (listener) => wrapListener(listener, { sample, maxBodyBytes: settings.maxBodyBytes, deliver, onError }),
    stats: () => coordinator.snapshot(),
    differences: () => coordinator.differences(),
    idle: async () => {
      while (inflight.size) await Promise.allSettled([...inflight]);
      await coordinator.idle();
    },
    close: () => coordinator.close(),
  };
}
```

- [ ] **Step 5: Run it and see it pass, on Windows and on Linux**

Run: `node --test --test-concurrency=1 twin/test/scenarios.test.js`, then `sh twin/scripts/test-linux.sh test/scenarios.test.js`

Expected: `# pass 2`, `# fail 0`.

- [ ] **Step 6: Run everything so far**

Run: `sh twin/scripts/test-linux.sh`

Expected: `# pass 18`, `# fail 0`.

- [ ] **Step 7: Commit**

```bash
git add twin
git commit -m "Twin: copy sampled real requests and their calls, and hand them to the ghosts"
```

---

### Task 7: The customer scenarios

**Files:**
- Create: `twin/test/apps/{blank-field,yen,open-door,double-charge,timestamps}/{old,new}.js`
- Modify: `twin/test/scenarios.test.js` (import line, and five tests at the end)

- [ ] **Step 1: Create the scenario apps**

`twin/test/apps/blank-field/old.js`:

```js
import { readJson, send } from '../http.js';

export function createApp() {
  return async (req, res) => {
    const policy = await readJson(req);
    send(res, 200, { region: policy.region ?? 'all', limit: policy.limits ? policy.limits.max : 100 });
  };
}
```

`twin/test/apps/blank-field/new.js`:

```js
import { readJson, send } from '../http.js';

// A new quota check that reads limits.max without checking that limits is there: it only breaks on policies with blank limits.
export function createApp() {
  return async (req, res) => {
    const policy = await readJson(req);
    const limit = policy.limits.max;
    send(res, 200, { region: policy.region ?? 'all', limit });
  };
}
```

`twin/test/apps/yen/old.js`:

```js
import { readJson, send } from '../http.js';

export function createApp() {
  return async (req, res) => {
    const { amount, currency } = await readJson(req);
    const total = currency === 'JPY' ? Math.round(amount * 0.9) : Math.round(amount * 90) / 100;
    send(res, 200, { currency, total });
  };
}
```

`twin/test/apps/yen/new.js`:

```js
import { readJson, send } from '../http.js';

// One discount formula for every currency: right for euros, but yen have no cents.
export function createApp() {
  return async (req, res) => {
    const { amount, currency } = await readJson(req);
    send(res, 200, { currency, total: Math.round(amount * 90) / 100 });
  };
}
```

`twin/test/apps/open-door/old.js`:

```js
import { send } from '../http.js';

export function createApp() {
  return (req, res) => {
    if (req.headers.authorization !== 'Bearer admin-token') return send(res, 401, { error: 'sign in first' });
    return send(res, 200, { users: 3 });
  };
}
```

`twin/test/apps/open-door/new.js`:

```js
import { send } from '../http.js';

// A tidied check that only refuses a wrong token, so a request with no token at all gets in.
export function createApp() {
  return (req, res) => {
    if (req.headers.authorization && req.headers.authorization !== 'Bearer admin-token') return send(res, 401, { error: 'sign in first' });
    return send(res, 200, { users: 3 });
  };
}
```

`twin/test/apps/double-charge/old.js`:

```js
import { readJson, send } from '../http.js';

export function createApp() {
  return async (req, res) => {
    const order = await readJson(req);
    const paid = await fetch(`${process.env.KERN_TEST_PAY_URL}/charge`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ amount: order.amount }) });
    send(res, 200, { paid: paid.status === 200 });
  };
}
```

`twin/test/apps/double-charge/new.js`:

```js
import { readJson, send } from '../http.js';

// Retries the charge unless the answer is 201 Created, but the payment service answers 200 OK: every order would be charged twice.
const charge = (amount) => fetch(`${process.env.KERN_TEST_PAY_URL}/charge`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ amount }) });

export function createApp() {
  return async (req, res) => {
    const order = await readJson(req);
    let paid = await charge(order.amount);
    if (paid.status !== 201) paid = await charge(order.amount);
    send(res, 200, { paid: paid.status === 200 });
  };
}
```

`twin/test/apps/timestamps/old.js`:

```js
import { randomUUID } from 'node:crypto';
import { send } from '../http.js';

// Every answer carries the time, a fresh id and a random number: real noise that must never count as a difference.
export function createApp() {
  return (req, res) => send(res, 200, { at: new Date().toISOString(), id: randomUUID(), roll: Math.random() });
}
```

`twin/test/apps/timestamps/new.js`:

```js
export { createApp } from './old.js';
```

- [ ] **Step 2: Write the failing tests**

In `twin/test/scenarios.test.js`, replace the line `import { production } from './helpers.js';` with:

```js
import { production, post, listen } from './helpers.js';
```

Then add at the end of the file:

```js
test('a crash on data only some requests carry is caught, and the customer still gets the old answer', async () => {
  const shop = await production('blank-field');
  assert.deepEqual(await (await post(shop.url, { region: 'eu', limits: { max: 7 } })).json(), { region: 'eu', limit: 7 });
  const answer = await post(shop.url, { region: '', limits: null });
  assert.equal(answer.status, 200);
  assert.deepEqual(await answer.json(), { region: '', limit: 100 });
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.deepEqual([stats.same, stats.different], [1, 1], JSON.stringify(stats));
  const [found] = shop.twin.differences();
  assert.ok(found.kinds.includes('crash'), JSON.stringify(found));
  assert.match(found.details[0].new, /TypeError/);
  await shop.close();
});

test('a rounding change on one currency is caught with the exact field', async () => {
  const shop = await production('yen');
  assert.deepEqual(await (await post(shop.url, { amount: 10, currency: 'EUR' })).json(), { currency: 'EUR', total: 9 });
  assert.deepEqual(await (await post(shop.url, { amount: 4501, currency: 'JPY' })).json(), { currency: 'JPY', total: 4051 });
  await shop.twin.idle();
  assert.equal(shop.twin.stats().same, 1);
  const [found] = shop.twin.differences();
  assert.deepEqual(found.kinds, ['body']);
  assert.deepEqual(found.details, [{ path: '$.total', old: '4051', new: '4050.9' }]);
  await shop.close();
});

test('a route that refused a stranger and would now let them in is a door that opened', async () => {
  const shop = await production('open-door');
  assert.equal((await fetch(`${shop.url}/admin`)).status, 401);
  await shop.twin.idle();
  const [found] = shop.twin.differences();
  assert.ok(found.kinds.includes('door-opened'), JSON.stringify(found));
  await shop.close();
});

test('a change that would charge twice is caught without charging anyone twice', async () => {
  let charges = 0;
  const pay = await listen((req, res) => {
    charges++;
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true}');
    });
  });
  process.env.KERN_TEST_PAY_URL = pay.url;
  try {
    const shop = await production('double-charge');
    assert.deepEqual(await (await post(shop.url, { amount: 25 })).json(), { paid: true });
    await shop.twin.idle();
    assert.equal(charges, 1);
    const [found] = shop.twin.differences();
    assert.ok(found.kinds.includes('effects'), JSON.stringify(found));
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_PAY_URL;
    await pay.close();
  }
});

test('time and random numbers never count as differences', async () => {
  const shop = await production('timestamps');
  await (await fetch(`${shop.url}/`)).arrayBuffer();
  await (await fetch(`${shop.url}/`)).arrayBuffer();
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.deepEqual([stats.same, stats.different, stats.notCompared], [2, 0, 0], JSON.stringify(stats));
  await shop.close();
});
```

- [ ] **Step 3: Run them**

Run: `node --test --test-concurrency=1 twin/test/scenarios.test.js`

Expected: `# pass 7`, `# fail 0`. The code from Tasks 1–6 already handles these. Each test checks one rule from the spec (R1, R2, R4, R6). If one fails, the failure message holds the verdict as JSON. Fix the code, never the test's expectation, unless the spec says otherwise.

- [ ] **Step 4: Run them on Linux**

Run: `sh twin/scripts/test-linux.sh test/scenarios.test.js`

Expected: `# pass 7`, `# fail 0`.

- [ ] **Step 5: Commit**

```bash
git add twin
git commit -m "Twin: catch a crash on blank data, a rounding change, an open door and a double charge"
```

---

### Task 8: Fail-safe behaviour

**Files:**
- Create: `twin/test/apps/{broken,hang,exits,slow,memory-hog}/{old,new}.js`
- Test: `twin/test/failsafe.test.js`

These tests cover every row of the failure table in spec section 8.

- [ ] **Step 1: Create the apps**

`twin/test/apps/broken/old.js`, `twin/test/apps/hang/old.js`, `twin/test/apps/exits/old.js`, `twin/test/apps/slow/old.js` and `twin/test/apps/memory-hog/old.js` all contain:

```js
export { createApp } from '../refactor/old.js';
```

`twin/test/apps/broken/new.js`:

```js
// A version that fails as soon as it loads.
throw new Error('this version does not load');
```

`twin/test/apps/hang/new.js`:

```js
// Never answers: a busy loop the ghost's time limit has to stop.
export function createApp() {
  return () => {
    for (;;) {
      // spin
    }
  };
}
```

`twin/test/apps/exits/new.js`:

```js
// Ends its own process; in a ghost that only ends the ghost's thread.
export function createApp() {
  return () => process.exit(3);
}
```

`twin/test/apps/slow/new.js`:

```js
import { createApp as createOld } from '../refactor/old.js';

// The same answers, 300 ms later: slow enough that a burst of requests fills the twin's queue.
export function createApp() {
  const old = createOld();
  return (req, res) => {
    setTimeout(() => old(req, res), 300);
  };
}
```

`twin/test/apps/memory-hog/new.js`:

```js
// Grows without end: the ghost's memory limit has to stop it.
export function createApp() {
  return () => {
    const hog = [];
    for (;;) hog.push(new Array(1e6).fill(1));
  };
}
```

- [ ] **Step 2: Write the tests**

`twin/test/failsafe.test.js`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tmpdir } from 'node:os';
import { wrapListener } from '../src/capture.js';
import { production, post, listen } from './helpers.js';

test('if the twin\'s own recording fails, the request is served as always and the twin is told', async () => {
  const problems = [];
  const listener = wrapListener((req, res) => res.end('ok'), {
    sample: () => { throw new Error('boom'); },
    maxBodyBytes: 1024,
    deliver: () => {},
    onError: (error) => problems.push(error.message),
  });
  const server = await listen(listener);
  assert.equal(await (await fetch(server.url)).text(), 'ok');
  assert.deepEqual(problems, ['boom']);
  await server.close();
});

test('an outgoing call whose answer is too large to copy leaves the request skipped, and the customer served', async () => {
  const pay = await listen((req, res) => {
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ok: true, note: 'x'.repeat(4096) }));
    });
  });
  process.env.KERN_TEST_PAY_URL = pay.url;
  try {
    const shop = await production('double-charge', { maxBodyBytes: 1024 });
    assert.deepEqual(await (await post(shop.url, { amount: 25 })).json(), { paid: true });
    await shop.twin.idle();
    assert.equal(shop.twin.stats().skipped, 1);
    await shop.close();
  } finally {
    delete process.env.KERN_TEST_PAY_URL;
    await pay.close();
  }
});

test('a new version that runs out of memory only ends its ghost', async () => {
  const shop = await production('memory-hog', { ghostMemoryMb: 64 });
  assert.equal((await fetch(`${shop.url}/?n=2`)).status, 200);
  await shop.twin.idle();
  assert.ok(shop.twin.differences()[0].kinds.includes('crash'), JSON.stringify(shop.twin.differences()));
  assert.equal(shop.twin.stats().disabled, null);
  await shop.close();
});

test('a ghost pair that keeps failing switches the twin off instead of restarting forever', async () => {
  const shop = await production('hang', { timeoutMs: 300, maxRestarts: 1 });
  for (const n of [1, 2]) {
    assert.equal((await fetch(`${shop.url}/?n=${n}`)).status, 200);
    await shop.twin.idle();
  }
  assert.match(shop.twin.stats().disabled, /restart more than 1 times/);
  assert.equal((await fetch(`${shop.url}/?n=3`)).status, 200);
  await shop.close();
});

test('a log file that cannot be written loses no difference', async () => {
  const shop = await production('renamed-field', { log: tmpdir() });
  await (await fetch(`${shop.url}/user`)).arrayBuffer();
  await shop.twin.idle();
  assert.equal(shop.twin.stats().logErrors, 1);
  assert.equal(shop.twin.differences().length, 1);
  await shop.close();
});

test('a new version that does not even load switches the twin off, and customers are served as always', async () => {
  const shop = await production('broken');
  assert.match(shop.twin.stats().disabled, /could not start[\s\S]*does not load/);
  assert.deepEqual(await (await fetch(`${shop.url}/?n=3`)).json(), { n: 3, total: 6 });
  await shop.close();
});

test('a new version that never answers is stopped, and the twin carries on', async () => {
  const shop = await production('hang', { timeoutMs: 500 });
  assert.deepEqual(await (await fetch(`${shop.url}/?n=3`)).json(), { n: 3, total: 6 });
  await shop.twin.idle();
  assert.ok(shop.twin.differences()[0].kinds.includes('hang'), JSON.stringify(shop.twin.differences()));
  assert.equal(shop.twin.stats().restarts, 1);
  assert.equal((await fetch(`${shop.url}/?n=4`)).status, 200);
  await shop.twin.idle();
  assert.equal(shop.twin.stats().different, 2);
  await shop.close();
});

test('a new version that ends its own process only ends its ghost', async () => {
  const shop = await production('exits');
  assert.equal((await fetch(`${shop.url}/?n=2`)).status, 200);
  await shop.twin.idle();
  assert.ok(shop.twin.differences()[0].kinds.includes('crash'));
  assert.equal(shop.twin.stats().disabled, null);
  await shop.close();
});

test('a request too large to copy is served and counted as skipped', async () => {
  const shop = await production('yen', { maxBodyBytes: 1024 });
  const answer = await post(shop.url, { amount: 1, currency: 'EUR', note: 'x'.repeat(4096) });
  assert.deepEqual(await answer.json(), { currency: 'EUR', total: 0.9 });
  await shop.twin.idle();
  assert.equal(shop.twin.stats().skipped, 1);
  await shop.close();
});

test('when the ghosts fall behind, customers are served as always and the overflow is counted', async () => {
  const shop = await production('slow', { queueLimit: 1 });
  const statuses = await Promise.all(Array.from({ length: 12 }, (_, i) => fetch(`${shop.url}/?n=${i}`).then((r) => r.status)));
  assert.ok(statuses.every((s) => s === 200));
  await shop.twin.idle();
  const stats = shop.twin.stats();
  assert.ok(stats.dropped > 0, JSON.stringify(stats));
  assert.equal(stats.sampled + stats.dropped, 12, JSON.stringify(stats));
  assert.equal(stats.same, stats.sampled, JSON.stringify(stats));
  await shop.close();
});

test('KERN_TWIN=off keeps the twin from starting at all', async () => {
  process.env.KERN_TWIN = 'off';
  try {
    const shop = await production('refactor');
    assert.equal(shop.twin.stats().disabled, 'switched off with KERN_TWIN=off');
    assert.equal((await fetch(`${shop.url}/?n=1`)).status, 200);
    await shop.close();
  } finally {
    delete process.env.KERN_TWIN;
  }
});
```

- [ ] **Step 3: Run them on Windows and on Linux**

Run: `node --test --test-concurrency=1 twin/test/failsafe.test.js`, then `sh twin/scripts/test-linux.sh test/failsafe.test.js`

Expected: `# pass 11`, `# fail 0`. These test rules R3, R5 and R7 and every row of the failure table in spec section 8. A failure here is a reliability bug: fix the code.

- [ ] **Step 4: Run the whole suite on Linux**

Run: `sh twin/scripts/test-linux.sh`

Expected: `# pass 34`, `# fail 0`.

- [ ] **Step 5: Commit**

```bash
git add twin
git commit -m "Twin: switch off, time out, restart and shed load without ever touching a customer's answer"
```

---

### Task 9: Measure the cost, write the README, run in CI

**Files:**
- Create: `twin/bench/overhead.js`, `twin/README.md`
- Modify: `.github/workflows/ci.yml` (add a step after line 36, `run: cargo test --locked`, in the `linux` job)

- [ ] **Step 1: Write the cost measurement**

`twin/bench/overhead.js`:

```js
// Measures what the twin adds to request latency: no twin, a twin that samples nothing, and one that samples 10% of requests.
import http from 'node:http';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../src/index.js';
import { createApp } from '../test/apps/refactor/old.js';

const app = (version) => ({ module: fileURLToPath(new URL(`../test/apps/refactor/${version}.js`, import.meta.url)), export: 'createApp' });

async function measure(label, listener, total = 4000, lanes = 16) {
  const server = http.createServer(listener);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${server.address().port}/?n=50`;
  const times = [];
  let sent = 0;
  const lane = async () => {
    while (sent < total) {
      sent++;
      const t = performance.now();
      await (await fetch(url)).arrayBuffer();
      times.push(performance.now() - t);
    }
  };
  await Promise.all(Array.from({ length: lanes }, lane));
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  times.sort((a, b) => a - b);
  const at = (q) => times[Math.min(times.length - 1, Math.floor(times.length * q))].toFixed(3);
  console.log(`${label.padEnd(24)} p50 ${at(0.5)} ms   p99 ${at(0.99)} ms`);
}

await measure('no twin', createApp());
for (const sampleRate of [0, 0.1]) {
  const twin = await createTwin({ old: app('old'), candidate: app('new'), sampleRate, queueLimit: 1000 });
  await measure(`twin sampling ${sampleRate * 100}%`, twin.wrap(createApp()));
  await twin.idle();
  console.log(`  ${JSON.stringify(twin.stats())}`);
  await twin.close();
}
```

- [ ] **Step 2: Run it on Linux and keep the output**

Run (WSL, repository root):

```bash
docker run --rm -v "$PWD/twin:/twin" -w /twin docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392 node bench/overhead.js
```

Expected: three latency lines (`no twin`, `twin sampling 0%`, `twin sampling 10%`), each followed by stats where `different` is 0. Copy the three latency lines exactly as printed for Step 3.

- [ ] **Step 3: Write the README**

Create `twin/README.md` with the content below. In the "Cost (measured)" section, put the three lines from Step 2 inside the `text` block, and today's date and `nproc` on that line.

````markdown
# Kern twin

The twin lets a new version of a Node service prove itself on real requests before any customer gets its answers.

It runs the old and the new version as ghosts. After the real answer has gone out, each ghost gets a copy of a sampled real request, in a sealed world where nothing real can be touched. Kern reports every difference, with the request and the exact place it differs.

This is Phase 1 of the design in `docs/superpowers/specs/2026-10-10-twin-runtime-design.md`. The twin compares and reports; it does not switch versions yet.

## Use

```js
import http from 'node:http';
import { createTwin } from './twin/src/index.js';
import { createApp } from './current/app.js'; // the version customers get today

const twin = await createTwin({
  old: { module: '/srv/shop/current/app.js', export: 'createApp' },   // the same code as the createApp above
  candidate: { module: '/srv/shop/next/app.js', export: 'createApp' }, // the change to check
  sampleRate: 0.05,
  log: '/var/log/kern-twin.jsonl',
});
http.createServer(twin.wrap(createApp())).listen(3000);
```

`createApp` must return a request listener, `(req, res) => { ... }`, and must not start a server itself. An Express app is such a listener; a Koa app gives one with `app.callback()`.

## What it never does

- **It never changes an answer a customer gets.**
- **It never lets a ghost touch anything real.** A ghost cannot reach the network, start a program or write a file. Its `fetch` calls get the real run's answers, and its writes are written down, never done.
- **It never breaks the app.** If anything goes wrong inside the twin, it switches itself off and says why in `twin.stats().disabled`. Setting `KERN_TWIN=off` keeps it from starting at all.

## Reading the results

`twin.stats()` counts every sampled request as one of:
- `same`;
- `different`;
- `notCompared`: the ghost needed something it may not use, or the old ghost did not reproduce the real run (reasons in `reasons`);
- `skipped`: too large to copy;
- `dropped`: the ghosts were busy.

It also gives `restarts`, `disabled`, and `logErrors` (difference lines the log file could not take).

`twin.differences()` and the `log` file hold one entry per difference:
- `method` and `url`;
- `kinds`: `crash`, `hang`, `door-opened`, `status`, `headers`, `body`, `effects`, `new-calls`, `fixed` or `changed-error`;
- `details`: `{ path, old, new }`.

## Options

| Option | Default | Meaning |
|---|---|---|
| `old` | required | `{ module, export }` of the version that serves customers |
| `candidate` | required | `{ module, export }` of the version to check |
| `sampleRate` | `1` | Share of requests compared, from 0 to 1 |
| `timeoutMs` | `5000` | How long a ghost may take for one request |
| `startMs` | `20000` | How long a ghost may take to load the app |
| `queueLimit` | `100` | Copies waiting to be compared; beyond it, requests are counted as dropped |
| `maxBodyBytes` | `1048576` | Largest body copied each way; larger requests are skipped |
| `ghostMemoryMb` | `512` | Memory limit of each ghost |
| `maxRestarts` | `20` | Ghost restarts before the twin switches itself off |
| `log` | none | File that gets one JSON line per difference |

## Not yet (Phase 1)

- **Server types.** Only `node:http` servers, without HTTP/2. Answers streamed longer than `maxBodyBytes` are skipped.
- **Outgoing calls.** `fetch` is the only call the ghosts replay. An app that talks to a database or another server any other way gets "not compared"; adapters come in Phase 4.
- **In-memory state.** Caches and counters inside the app differ between the real app and the ghosts, so such requests may be "not compared".
- **Not yet built:** switching versions on, naming the changed code, and fixing (Phases 2, 3 and 5).

## Cost (measured)

Measured with `node bench/overhead.js` in `node:22-bookworm-slim` on (date, number of cores from `nproc`):

```text
(the three latency lines from the bench)
```
````

- [ ] **Step 4: Run the tests in CI**

In `.github/workflows/ci.yml`, in the `linux` job, add this step right after:

```yaml
      - name: Test
        run: cargo test --locked
```

The new step:

```yaml
      - name: Twin runtime
        run: sh twin/scripts/test-linux.sh
```

- [ ] **Step 5: Check the CI step's indentation, and run the full suite once more**

Run:

```bash
sed -n '33,40p' .github/workflows/ci.yml
sh twin/scripts/test-linux.sh
```

Expected:
- The new `- name: Twin runtime` line starts with exactly 6 spaces, the same as `- name: Test` above it.
- Its `run:` line starts with 8 spaces.
- Then `# pass 34`, `# fail 0`.

- [ ] **Step 6: Commit**

```bash
git add twin .github/workflows/ci.yml
git commit -m "Twin: measure its cost, document it, run its tests in CI"
```

---

### Task 10: Finish

- [ ] **Step 1: Run everything on both systems**

Run: `node --test --test-concurrency=1 "twin/test/**/*.test.js"` (Windows), then `sh twin/scripts/test-linux.sh` (WSL)

Expected: `# pass 34`, `# fail 0` on both.

- [ ] **Step 2: Check Phase 1 against the spec**

Go through spec section 12, Phase 1. Tick each item and name the test that proves it:

1. All tests pass on Linux and Windows (Step 1).
2. Five scenarios caught, customer answers untouched (`scenarios.test.js`).
3. Time and randomness never show as differences (`time and random numbers never count as differences`).
4. Fail-safe tests pass (`failsafe.test.js`).
5. Cost measured and written (`twin/README.md`).

Go through spec section 3. Every rule R1–R8 must point to a test or a review note; R8 is "code review" in Phase 1.

- [ ] **Step 3: Update memory**

In `C:\Users\krish\.claude\projects\c--Users-krish-Downloads-Kern\memory\kern-first-principles-inventions.md`, add:
- that Phase 1 is built, on which branch and commits;
- the measured cost lines;
- anything learned that the next phase must know.

- [ ] **Step 4: Report to the user in simple English**

Say:
- what now works, with one example: "the double-charge change was caught and nobody was charged twice";
- the measured cost;
- what is not done yet.

Then ask whether to push, or to start the Phase 2 plan (switching on, from spec section 12). Do not push without a clear yes.
