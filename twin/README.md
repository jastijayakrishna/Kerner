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
- **It never lets a ghost touch anything real through Node's own APIs.** A ghost cannot open a network connection, start a program or a thread, write a file, or open one to write. Its `fetch` calls get the real run's answers, and its writes are written down, never done. Its console output and logs are dropped, never mixed into the app's logs.
- **It never breaks the app.** If anything goes wrong inside the twin, it switches itself off and says why in `twin.stats().disabled`. The ghosts never keep the app's process alive. Setting `KERN_TWIN=off` keeps the twin from starting at all.

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
- **Preloaded code.** Ghosts start with the app's Node flags, so modules preloaded with `--require`, `--import` or `NODE_OPTIONS` also load in each ghost, before the seal goes on. A preloaded agent (an APM agent, say) starts in the ghosts too.
- **Below Node's APIs.** The seal is built from Node's own APIs. Native add-ons and WebAssembly system calls can act below it, and DNS lookups are not refused.
- **Not yet built:** switching versions on, naming the changed code, and fixing (Phases 2, 3 and 5).

## Cost (measured)

Measured with `node bench/overhead.js` in `node:22-bookworm-slim` (Node 22.23.3) on 2026-10-10, 20 cores (`nproc`). The bench sends 4,000 requests to a tiny endpoint over 16 connections at once, from the same Node process as the server, after one unreported warm-up round:

```text
no twin                  p50 1.555 ms   p99 8.405 ms
twin sampling 0%         p50 1.398 ms   p99 6.762 ms
twin sampling 10%        p50 2.686 ms   p99 8.253 ms
```

Three runs gave a median of 1.35–1.56 ms with no twin, 1.33–1.40 ms with a twin that samples nothing, and 2.63–2.96 ms with a twin that samples 10%, all with no difference found. So a twin that samples nothing costs nothing measurable. Sampling 10% roughly doubled the median here, because each sampled request costs extra work on the app's main thread. Keep `sampleRate` low until that cost comes down.
