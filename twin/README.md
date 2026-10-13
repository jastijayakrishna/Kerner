# Kern twin

The twin lets a new version of a Node service prove itself on real requests, and lets it serve customers only once it has.

It runs the old and the new version as ghosts:
- After a real answer has gone out, both ghosts get a copy of the sampled request, in a sealed world where nothing real can be touched.
- Kern compares their answers and their writes, and counts every comparison per customer segment and route.
- Once the new version has enough clean comparisons to bound its risk, it may serve.
- The first unexplained difference while it serves switches serving back to the old version, before the next request.

Phases 1 to 6 of `docs/superpowers/specs/2026-10-10-twin-runtime-design.md` are built: shadow comparisons, evidence and switch-on, changed lines and named causes, real stacks, the fix loop, and the proof on real apps.

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
  - cannot write a file (with `overlay: true` its writes land in memory instead), open one to write, or write to a file descriptor it did not open.

  Its `fetch` calls, and with `adapters` its `node:http`, Postgres and Redis calls, get the real run's answers. Its writes are written down, never done. Its output is dropped, and it starts without the app's preloads (`--require`, `--import`, `NODE_OPTIONS`).
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

## Changed lines and the pull request behind a difference

With a manifest from `kern twin plan`, the twin knows which changed lines of code each request ran:

```bash
kern twin plan . "$BASE_SHA" "$HEAD_SHA" .kern-twin/manifest.json
```

```js
const twin = await createTwin({
  old: { module: '/srv/shop/current/src/app.js', export: 'createApp', root: '/srv/shop/current' },
  candidate: { module: '/srv/shop/next/src/app.js', export: 'createApp', root: '/srv/shop/next' },
  manifest: '/srv/shop/next/.kern-twin/manifest.json',
  minLineRuns: 3,
});
```

- **Each version's `root` is its checkout,** the folder the manifest's paths start from. At start the twin checks that every changed line's text is at its line under both roots. If it is not, the twin switches off and names the line, so a manifest for other code is never applied.
- **Both ghosts measure their side of the change** with V8's precise coverage: the old ghost the lines the change removed or replaced, the new ghost the lines it added. Code built from other files (TypeScript, bundles) is read through its source map.
- **A line is proven** when every code character on it has run in at least `minLineRuns` clean comparisons. So both branches of `a ? b : c` must run before the line counts. With a manifest, switching on also waits for every changed line, old and new, to be proven, to have run while the app loaded, or to be listed in `accept` (for example `'old:src/app.js:17'` or `'new:src/app.js:20-24'`).
- **`twin.paths()`** lists every changed line as `never run`, `partly run`, `run`, `proven`, `at load` or `accepted`, with its pull request. `kern-twin status --manifest <file>` prints the same.
- **Each difference lists `pulls`,** the pull requests whose changed lines the request ran, and names a `cause` when exactly one did.

Measuring lines needs the inspector, so it runs in thread ghosts. Its cost falls on the ghosts, never on the customers' requests. Measured with `node bench/ghosts.js` in `node:22-bookworm-slim` (Node 22.23.3) on 2026-10-11, 20 cores; the middle of three runs:

```text
without measuring lines      1149 requests judged per second (600 same, 0 different)
measuring changed lines       228 requests judged per second (600 same, 0 different)
```

Over the three runs: 1,094–1,347 without, 224–230 with, so the ghosts judge about 5 times fewer requests per second. The cost is reading the counts after each request, not running with coverage on. A loopback request like the ghosts' own took 0.79 ms with coverage off, 0.82 ms with it on, and 4.13 ms with the counts read after each request: each read reports every function that ran since the last one, Node's own HTTP code included. When the ghosts fall behind, the twin compares fewer requests (`dropped` counts the rest), and the evidence takes longer to gather.

## Real stacks

Each ghost replays these client libraries from the real run's recording:

| Name | What it covers |
|---|---|
| `fetch` | always on |
| `http` | `node:http` and `node:https` clients, so axios, got, node-fetch and other libraries built on them |
| `pg` | node-postgres: `Client`, `Pool`, transactions |
| `ioredis` | commands, pipelines and transactions; not Redis Cluster or subscriber mode |

```js
const twin = await createTwin({ ...versions, adapters: ['http', 'pg', 'ioredis'] });
```

- **Matching by shape.** Every call is named by its shape and its payload. A ghost whose payload carries the clock or a fresh id (an `INSERT ... created_at`) gets the recorded call of the same shape. Writes are checked against the real run by shape. The two ghosts are still compared exactly, since they share one world.
- **Real reads.** `realReads: ['GET https://rates.example/*']` lets the twin's own thread make those reads for real, once per request, when a cold ghost needs one the warm real app did not. Both ghosts get the same answer. Only GET and HEAD, at most 5 s. A read must go to the very origin a pattern names (scheme, host and port), and its path is matched after `..` is resolved, as `fetch` resolves it; a pattern is a whole http or https address.
- **File overlay.** With `overlay: true`, a ghost's file writes land in memory for the request. They are written down, read back by the ghost, and never touch the disk.
- **Process ghosts.**
  - With `ghosts: 'process'`, each ghost is a Node process under the permission model: Node itself refuses file writes, programs, threads, native add-ons and the inspector. This stops even code that runs before the JavaScript seal, such as a module given in `ghostExecArgv`.
  - On Linux, `sandbox: 'bwrap'` puts each process ghost in a namespace with no network and a read-only file system. Changed lines are still measured there.
  - Without `bwrap`, a twin with a manifest needs thread ghosts.
- **The old version beside the new one.**

  ```bash
  node twin/bin/kern-twin.js build --repo . --base "$BASE_SHA" --out .kern-twin/old
  ```

  It shares the new version's `node_modules` when no package file changed, and otherwise installs the old version's own.
- **The acceptance run** (`twin/examples/shop/run.sh`): an Express and Postgres shop under the twin. Measured on 2026-10-11:

  ```text
  sampled 200: same 190, different 10 (large orders' rounded totals), not compared 0, skipped 0, dropped 0, restarts 0
  PASSED: the twin compared 100.0% of 200 sampled requests
  ```

## From a difference to a fix

With `cases: '.kern-twin/cases'`, every unexplained difference becomes a case:
- **Scrubbed first.** Private values become stand-ins of the same shape, and within a case the same value always gets the same stand-in. The stand-ins are keyed by a secret that is thrown away, so nothing links two cases. This covers:
  - emails, card numbers (a valid test number for a valid one), UUIDs (a valid UUID) and tokens, anywhere;
  - values under names such as `password` or `card`, in JSON, forms and query strings;
  - private headers.
- **Checked again.** Both versions run again on the scrubbed copy. The case is written only if the difference shows again. If it does not, the difference depended on a private value, and a stub tells a person to look.
- **Everything else on disk is scrubbed too:** the differences log and the evidence's examples. Only `twin.differences()`, in memory, keeps the real values. A path segment with an `@` inside counts as an id, so no email reaches the disk in a route name.
- **Limits.**
  - A body the scrubber cannot read (binary, a file upload) is kept as it was and named in the case's `opaque` list, so a person looks before the case is shared.
  - In other text (XML, say) only the patterns apply, not the private names.
  - A value the app adds from its own configuration (a secret from the environment in an error message, say) is not a request value, and is not scrubbed.

Then:

```bash
npx kern-twin test --cases .kern-twin/cases --candidate src/app.js          # fails until the new code answers as the old one did
npx kern-twin feedback --cases .kern-twin/cases --candidate src/app.js      # kern-feedback/v1 instructions for a coding agent
npx kern-twin comment --cases .kern-twin/cases --head "$HEAD_SHA"           # the pull request comment (printed, never posted)
```

- **What the agent reads.** `feedback` and the comment give the agent's copy of the verdict. As in Kern, it leaves out the hints on how a person could allow a difference, so an agent fixes its change instead of allowing it. The comment carries its own marker, `<!-- kern-twin:v1 head=… -->`, so Kern's count of rejected attempts is unchanged.
- **A person decides** about an opened door, about writes to `humans.hosts` (a payment provider, say), and about stubs. These make `agent_fixable` false.
- **A known difference is never forgotten.** Before any later candidate may switch on, the twin runs every case on both versions, one at a time and in turn with real requests.
  - A case that still shows a difference holds it.
  - So does a stub, until a person has looked and removed its file.
  - So does a case file that cannot be read.
  - A case the twin can no longer run is only listed in `twin.evidence().known`. Live traffic must then prove that path.
  - `twin.idle()` also waits until the known cases are checked.
- **A fix earns its evidence again.** It is a new candidate, and nothing carries over from the one it fixes. Code that did not change can still behave differently after a fix, through state the fixed code leaves behind for later requests.

## Real apps

- **An app that reads while it starts** (its database, its cache, its key store) needs `load: 'record'`. The old version then starts in the app's process inside a record of its own, and the ghosts start from those answers. A new version that asks at its start for something the old one never did is named, call by call.
- **Background work stays out of the comparison.** Inside a ghost, only the request being answered gets time and the outside world. Work an app leaves running (a cron tick, a queue's polling loop, a retry) waits forever there, so it never mixes into another request.
- **An app that does not export a listener** needs a small entry module that builds it without listening. See `twin/proof/infisical/twin-entry.mjs` and `twin/proof/firecrawl/twin-entry.mjs`.
- **A read the new code makes that the old never did** can be answered for real, read-only, with `pgReads: '<connection string>'` (Postgres; off by default; best pointed at a read replica or a read-only role). Every such read runs inside `BEGIN TRANSACTION READ ONLY`, so a write is refused by the database itself.
- **What it took, and what it found, on two real apps:** `reports/twin-proof-<date>.md`.

## Reading the results

`twin.stats()` counts, for this run:
- **`same`, `stated` and `different`.** `stated` means different only where `allow` says it may be.
- **`notCompared`,** with its reasons in `reasons`. It covers three cases:
  - both versions needed something a ghost may not use (when only the new code reaches for it, that is a difference of kind `refused`);
  - one ghost read something the recording cannot answer (`unanswered`);
  - the serving version's ghost did not reproduce the real run.
- **`skipped`:** too large to copy, or aborted by the client.
- **`dropped`:** the twin was busy.
- **The rest:** `restarts`, `logErrors`, `evidenceErrors`, `cases` and `caseErrors` (cases written, and cases that could not be written), `queued`, `disabled`, `serving`, and `adapters` (whether each client library is being recorded).

`twin.differences()` holds the first 100 unexplained differences of this run, with their real values. The `log` file holds all of them, scrubbed, one JSON line each:
- `n`, `at`, `route`, `segment`, `serving`, `method` and `url`;
- `kinds`: `crash`, `hang`, `door-opened`, `status`, `headers`, `body`, `effects`, `refused` (the new code reached for something a ghost may not use, such as a program or a connection, which the old code did not), `fixed` or `changed-error`;
- `details`: `{ kind, path, old, new }`.

`twin.evidence()` gives the counts across restarts, per segment.

## Options

| Option | Default | Meaning |
|---|---|---|
| `old` | required | `{ module, export }` of the version that serves customers today; `module` is a path (a relative one is read from the working folder) or a file URL |
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
| `manifest` | none | The file `kern twin plan` wrote |
| `old.root`, `candidate.root` | required with `manifest` | Each version's checkout |
| `minLineRuns` | `3` | Clean runs every code character of a changed line needs (1 to 9) |
| `accept` | `[]` | Changed lines a person accepts without runs, such as `'old:src/x.js:12'` |
| `adapters` | `['fetch']` | Client libraries recorded and replayed: `'http'`, `'pg'`, `'ioredis'` |
| `load` | `'none'` | `'record'`: the ghosts start from the record of the app's own start (for apps that read while they start) |
| `realReads` | `[]` | Reads the twin may make for real for the ghosts, such as `'GET https://rates.example/*'` |
| `pgReads` | none | A Postgres connection string; a read the new code makes that the old never did is answered for real, read-only (needs the `'pg'` adapter) |
| `overlay` | `false` | Ghost file writes land in memory for the request |
| `ghosts` | `'thread'` | `'process'`: each ghost in its own Node process, under the permission model |
| `sandbox` | `'none'` | `'bwrap'` (Linux, process ghosts): no network and a read-only file system |
| `cases` | none | Folder for scrubbed, re-checked cases; every later candidate must pass them |
| `scrub` | defaults | More private `headers`, and `keys` (value names in JSON, forms and query strings), to scrub: `{ headers: [...], keys: [...] }` |
| `humans` | `{ kinds: ['door-opened'] }` | Kinds and write hosts a person decides about: `{ kinds, hosts }` |

## Not yet

- **Server types.** Only `node:http` servers, without HTTP/2. Answers streamed longer than `maxBodyBytes` are skipped.
- **Outgoing calls.** `fetch`, `node:http`/`https` clients, `pg` and `ioredis` are replayed. Other clients (MySQL, MongoDB, Redis Cluster, undici's own client) are refused in a ghost, so a request that uses one is not compared.
- **In-memory state.** Caches and counters inside the app differ between the real app and the ghosts, so such requests may be "not compared".
- **Below Node's APIs.** Thread ghosts are sealed in JavaScript only. Process ghosts add Node's permission model, and `bwrap` adds the kernel's network and file isolation.
- **Two versions in one process.** Once the new version serves, both versions are loaded in the app's process. An app whose `createApp` starts background work, or registers process-wide singletons, should use `wrap()` and switch with its deploy system, reading `kern-twin status`.

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
