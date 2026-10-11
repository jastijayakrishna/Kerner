# Kern twin

The twin lets a new version of a Node service prove itself on real requests, and lets it serve customers only once it has.

It runs the old and the new version as ghosts:
- After a real answer has gone out, both ghosts get a copy of the sampled request, in a sealed world where nothing real can be touched.
- Kern compares their answers and their writes, and counts every comparison per customer segment and route.
- Once the new version has enough clean comparisons to bound its risk, it may serve.
- The first unexplained difference while it serves switches serving back to the old version, before the next request.

Phases 1 (shadow comparisons) and 2 (evidence and switch-on) of `docs/superpowers/specs/2026-10-10-twin-runtime-design.md` are built.

## Use

```js
import http from 'node:http';
import { createTwin } from './twin/src/index.js';

const twin = await createTwin({
  old: { module: '/srv/shop/current/app.js', export: 'createApp' },   // the version customers get today
  candidate: { module: '/srv/shop/next/app.js', export: 'createApp' }, // the change to prove
  sampleRate: 0.05,
  stateDir: '/var/lib/kern-twin',   // which version serves, and the evidence, kept across restarts
  switchOn: 'auto',                 // or 'manual' (the default): a person switches with kern-twin serve
  epsilon: 0.001,                   // switch on once an unexplained difference is rarer than 1 in 1,000, with 95% confidence
  minHours: 24,                     // and not before a full day of traffic
  log: '/var/log/kern-twin.jsonl',
});
http.createServer(await twin.listener()).listen(3000);
```

`createApp` must return a request listener, `(req, res) => { ... }`, and follow three rules. An Express app is such a listener; a Koa app gives one with `app.callback()`.
- **No server of its own.**
- **No background work** (timers, queue consumers). The old version is loaded in the app's thread, and so is the new one once it serves, so two copies can live in one process.
- **Safe to load twice in one process.** For example, it registers metrics only once.

`twin.wrap(listener)` still works the Phase 1 way: it compares around the app's own listener, and the old version always serves.

## What it never does

- **It never serves the new version before the new version has earned it**, unless a person switches it on.
- **It never lets a ghost touch anything real through Node's own APIs.** A ghost:
  - cannot open a network connection, send UDP or look up a name;
  - cannot start a program or a thread, signal a process, or change a process's priority;
  - cannot load a native add-on (except packages listed in `addons`), use Node's internal bindings, or start WASI;
  - cannot write a file, open one to write, or write to a file descriptor it did not open.

  Its `fetch` calls get the real run's answers. Its writes are written down, never done. Its output is dropped, and it starts without the app's preloads (`--require`, `--import`, `NODE_OPTIONS`).
- **It never breaks the app.** If anything goes wrong inside the twin, it switches itself off and says why in `twin.stats().disabled`, and the old version serves. The twin's thread and the ghosts never keep the app's process alive by themselves; only `await twin.idle()` does, until the twin has judged what it was handed. `KERN_TWIN=off` keeps the twin from starting at all; the old version then serves.
- **It never slows the app's thread with judging.** Comparing, counting and deciding happen in the twin's own thread. The app's thread only copies sampled requests, and hands each over as exact bytes: never a slice of Node's shared buffer pool, which would carry other requests' data along.

## Switching on, and back

- **The rule of three.** After n comparisons with no unexplained difference, the true rate of unexplained differences is below 3/n with 95% confidence. With `epsilon: 0.001`, the new version needs 3,000 clean comparisons.
- **The decision** (`twin.decision()`) is ready when all of these hold:
  - there is no unexplained difference;
  - every segment listed in `segments` has at least ceil(3/ε) clean comparisons (`same` or `stated`); `'*'` is all traffic;
  - at most `maxUncovered` of its requests were not compared or skipped;
  - at least `minHours` have passed since the first comparison.

  When it is not ready, `reasons` says why.
- **Switching on.** With `switchOn: 'auto'`, the new version is switched on as soon as the decision is ready. With `'manual'` (the default), a person does it: `twin.serve('candidate')`, or `kern-twin serve candidate --state <dir>`.
- **Switching back.** The first unexplained difference while the new version serves switches serving back to the old version before the next request. After any switch back, the twin does not switch on by itself again until a person switches the new version on.
- **Allowed differences.** A difference the team means is stated with `allow`, for example `{ route: 'GET /user', paths: ['$.user_name', '$.username'] }`:
  - every detail must be covered;
  - a crash, a hang, an opened door or another write can only be stated by naming its kind (`kinds: ['crash']`).

## The state folder

With `stateDir`, the twin keeps two files:
- **`control.json`:** which version serves, whether switching on by itself is held, and when and why serving last changed. Edit it, or use `kern-twin serve`; a running twin follows within a second.
- **`evidence.json`:** the counts per segment and route for the current candidate, with a sha256 checksum. A damaged file, or one for other code, starts again and says why in `twin.evidence().note`.

A candidate is named by its code: the sha256 of its package's code files, or `candidate.id`. Any change to the code starts its evidence again.

To print the evidence and the decision: `node twin/bin/kern-twin.js status --state <dir>`.

## Reading the results

`twin.stats()` counts, for this run:
- **`same`, `stated` and `different`.** `stated` means different only where `allow` says it may be.
- **`notCompared`,** with its reasons in `reasons`. It covers three cases:
  - a ghost needed something it may not use;
  - one ghost read something the recording cannot answer (`unanswered`);
  - the serving version's ghost did not reproduce the real run.
- **`skipped`:** too large to copy, or aborted by the client.
- **`dropped`:** the twin was busy.
- **The rest:** `restarts`, `logErrors`, `evidenceErrors`, `queued`, `disabled` and `serving`.

`twin.differences()` holds the first 100 unexplained differences of this run. The `log` file holds all of them, one JSON line each:
- `n`, `at`, `route`, `segment`, `serving`, `method` and `url`;
- `kinds`: `crash`, `hang`, `door-opened`, `status`, `headers`, `body`, `effects`, `fixed` or `changed-error`;
- `details`: `{ kind, path, old, new }`.

`twin.evidence()` gives the counts across restarts, per segment.

## Options

| Option | Default | Meaning |
|---|---|---|
| `old` | required | `{ module, export }` of the version that serves customers today |
| `candidate` | required | `{ module, export, id? }` of the version to prove |
| `sampleRate` | `1` | Share of requests compared, from 0 to 1 |
| `timeoutMs` | `5000` | How long a ghost may take for one request |
| `startMs` | `20000` | How long a ghost may take to load the app |
| `queueLimit` | `100` | Copies waiting to be compared; beyond it, requests are counted as dropped |
| `maxBodyBytes` | `1048576` | Largest body copied each way; larger requests are skipped |
| `ghostMemoryMb` | `512` | Memory limit of each ghost |
| `maxRestarts` | `20` | Ghost restarts before the twin switches itself off |
| `log` | none | File that gets one JSON line per difference |
| `route` | the path, ids replaced | `(req) => 'GET /users/:id'`: how requests are grouped |
| `segment` | `'all'` | `(req) => 'pro'`: the customer group a request belongs to |
| `allow` | `[]` | Differences the team means, as `{ route, paths?, kinds? }` |
| `stateDir` | none | Folder for `control.json` and `evidence.json` |
| `epsilon` | `0.001` | The risk bound for switching on |
| `minHours` | `24` | Hours of traffic before switching on |
| `segments` | `['*']` | Segments that each need the bound; `'*'` is all traffic |
| `maxUncovered` | `0.05` | Largest share of requests that may go uncompared |
| `switchOn` | `'manual'` | `'auto'`: switch on as soon as the decision is ready |
| `ghostExecArgv` | `[]` | Node flags for the ghosts, such as a TypeScript loader; they run before the seal |
| `addons` | `[]` | Packages whose native add-ons a ghost may load (pure computation only, such as `bcrypt`) |

## Not yet

- **Server types.** Only `node:http` servers, without HTTP/2. Answers streamed longer than `maxBodyBytes` are skipped.
- **Outgoing calls.** `fetch` is the only call the ghosts replay. An app that talks to a database or another server any other way gets "not compared"; adapters come in Phase 4.
- **In-memory state.** Caches and counters inside the app differ between the real app and the ghosts, so such requests may be "not compared".
- **Below Node's APIs.** The seal is built from Node's own APIs. A native add-on allowed with `addons` acts below it. Separate ghost processes with Node's permission model and an OS sandbox come in Phase 4.
- **Two versions in one process.** Once the new version serves, both versions are loaded in the app's process. An app whose `createApp` starts background work, or registers process-wide singletons, should use `wrap()` and switch with its deploy system, reading `kern-twin status`.
- **Not yet built:**
  - naming the changed code and the pull request behind a difference (Phase 3);
  - real stacks (Phase 4);
  - fixing (Phase 5).

## Cost (measured)

Measured with `sh scripts/bench-linux.sh` in `node:22-bookworm-slim` (Node 22.23.3) on 2026-10-11, 20 cores. The bench sends 4,000 requests to a tiny endpoint over 16 connections at once, from the same Node process as the server, after one unreported warm-up round:

```text
no twin                  p50 1.440 ms   p99 6.393 ms
twin sampling 0%         p50 1.242 ms   p99 6.456 ms
  compared 0 (same 0), not compared 0, skipped 0, dropped 0
twin sampling 10%        p50 2.314 ms   p99 9.509 ms
  compared 409 (same 409), not compared 0, skipped 0, dropped 0
twin sampling 100%       p50 2.274 ms   p99 7.907 ms
  compared 4000 (same 4000), not compared 0, skipped 0, dropped 0
```

| Setup | Median p50 over three runs | p99 range |
|---|---|---|
| No twin | 1.409 ms | 5.46–8.20 ms |
| Twin sampling 0% | 1.272 ms | 4.99–6.46 ms |
| Twin sampling 10% | 2.042 ms | 7.38–9.51 ms |
| Twin sampling 100% | 2.185 ms | 6.61–7.91 ms |
| Twin sampling 10%, `--experimental-async-context-frame` | 1.537 ms (one run; 1.241 ms without a twin in that run) | 4.89 ms |

A twin that samples nothing costs nothing measurable. Sampling 10% costs about 0.6 ms at the median, 1.45 times the median without a twin (1.6 times in the slowest run). That is less than in Phase 1, where 10% gave a p50 of 2.63–2.96 ms against 1.35–1.56 ms without a twin. Sampling 100% costs about the same as 10%, so most of the cost is fixed, not per request: it is Node 22's default `AsyncLocalStorage`, which slows every promise in the process once a sampled request has used it. With `--experimental-async-context-frame` (the default from Node 24), sampling 10% costs 1.54 ms against 1.24 ms. In a CPU profile of the app's thread over a whole run, the twin's own functions are 2.6% of its busy time, mostly `capture.js` (`finish`, `begin`) and `route.js` (`routeOf`); none is among its 15 costliest functions.
