# Twin runtime: design

Date: 2026-10-10.

Status:
- Direction agreed with the user in the brainstorm of 2026-10-10.
- Phases 1 to 6 are built (branch `twin`). Their plans are in `docs/superpowers/plans/2026-10-10-twin-runtime-phase-1.md`, `-phase-2.md`, `-phase-3.md`, `-phase-4.md`, `-phase-5.md` and `-phase-6.md`.
- Phases 3 to 6 each have their own plan in the same folder.
- `docs/superpowers/specs/2026-10-10-twin-runtime-interfaces.md` pins the contracts between phases: files, messages, shapes, options and formats.

## 1. The promise

An AI agent changes code in a team's Node service. The twin makes the change prove itself on real customer requests before any customer gets its answers.

- The new version runs as a ghost beside the version that serves customers today.
- It gets the exact same requests, and it can touch nothing real.
- Kern compares the two answers and the two lists of actions.
- A difference is caught before anyone is hurt, and comes with the exact request, the old answer and the new answer.

Later phases add three things:
- **Earned switch-on.** A change switches on only after enough clean comparisons; three over n gives the risk bound.
- **Named causes.** Each difference is tied to the exact changed code and the pull request that changed it.
- **A fix loop.** The failing case becomes a test the team's own coding agent fixes, and the fix must prove itself the same way.

## 2. One request's life (Phase 1, shadow mode)

1. **A request arrives.** The twin samples it (`sampleRate`). Requests that are not sampled go straight to the app, untouched.
2. **The real app serves it exactly as always.** Meanwhile the twin keeps copies:
   - the request body, as the HTTP parser hands it to the app;
   - the response, as the app writes it;
   - every `fetch` the app makes: what was asked and what came back.
3. **After the answer has gone out, the copies go to two ghosts.** These are worker threads, one holding the old version of the app and one holding the new.
4. **Each ghost replays the request against its version through real HTTP on loopback, in a sealed world:**
   - the clock is the moment the real request arrived;
   - random numbers come from one seed per request;
   - `fetch` calls are answered from the recording;
   - writes are written down, not done;
   - anything else that would reach outside is refused: sockets, programs, file writes.
5. **Kern checks the old ghost against reality.** Did it give the real status and make the real writes? If not, the request is "not compared", and the reason is counted.
6. **Kern compares the two ghosts:**
   - status;
   - headers that mean something;
   - the body, by value for JSON and text;
   - the writes.

   A difference is recorded with its kind, the paths that differ and the request. The customer's answer was never touched.

## 3. Rules the twin must never break

| Rule | How it is kept | The test that tries to break it |
|---|---|---|
| R1. Customers get exactly the answers of the version serving them: the old version, until the new one has earned it or a person switched it on | The wrapper only observes; the router sends each request whole to one version; the twin's thread asks to switch on only when `decide()` is ready, and switches back at the first unexplained difference | Every scenario test checks the customer's answer; `switch.test.js` |
| R2. A ghost never touches anything real | `ghost-io.js` denies by default: fetch is answered from the recording; sockets, UDP, programs and file writes are refused | `ghost-io.test.js`; the double-charge scenario counts real charges |
| R3. The twin failing never reaches a customer | Every hook sits in try/catch, and any internal error switches the twin off. Ghosts are separate threads with memory limits; a hung ghost is terminated | `failsafe.test.js` covers a broken version, a hang, an exiting ghost, an oversized body, a full queue and the off switch |
| R4. Both ghosts live in the same world | `world.js` resets clock and seed per request; both get the same recording; they process the same requests in the same order and always restart together | The timestamps scenario: time and random values never show up as differences |
| R5. Nothing is called "same" without being compared | Every request ends as same, stated, different, not compared or skipped, and aborted requests are skipped too. A read the recording cannot answer makes a request not compared. Not compared and skipped requests are counted with reasons, never folded into "same", and too many of them hold switch-on back (`maxUncovered`) | The compare tests (`unsupported`, `unanswered`); `capture.test.js` (aborted); the fail-safe tests (`skipped`, `dropped`); `decide.test.js` |
| R6. Every difference names its request | Recorded with method, URL, kinds and the exact paths | Scenario tests check paths such as `$.total` |
| R7. The cost is bounded | Sampling; a queue limit that sheds load; a body cap; a memory cap per ghost and for the twin's thread; one request at a time per ghost pair; judging in the twin's own thread, never on the app's | The queue-limit test; `bench/overhead.js` measures the cost |
| R8. Recordings stay in the process | Ghosts have no network. Bytes cross threads as exact copies, never as slices of Node's shared buffer pool. The differences log and the state folder are local files. Cases, the differences log and the evidence's examples are scrubbed; only `differences()` in memory keeps real values | `ghost-io.test.js` (no network); `bytes.test.js`; `scrub.test.js`; `casefiles.test.js` (nothing private reaches the disk) |

## 4. The pieces

Everything lives in `twin/`, a TypeScript-free Node 22 ESM package with no dependencies.

| File | Its one job | Uses |
|---|---|---|
| `src/keys.js` | Names an outgoing call the same way everywhere (method, URL, short body hash), and says whether a method writes | `node:crypto` |
| `src/serialize.js` | Unpacks compressed bodies, reads JSON or text by value, and lists differing paths | `keys.js` |
| `src/compare.js` | Compares two runs into same, different or not compared with kinds; checks that the old ghost reproduced reality | `serialize.js` |
| `src/world.js` | The ghost's clock and random numbers (ported from Kern's `probe.cjs` `steady()`), reset per request | Node globals |
| `src/ghost-io.js` | What a ghost may do: replay fetch, write down writes, refuse the rest | `keys.js` |
| `src/worker.js` | One ghost: loads one version of the app, serves it on loopback, runs jobs | `world.js`, `ghost-io.js` |
| `src/coordinator.js` | Runs the ghost pair: starts, asks, times out, restarts together, judges, counts and remembers | `worker.js`, `compare.js` |
| `src/capture.js` | Wraps the real request listener, copies what goes in and out, holds the per-request record | `node:async_hooks` |
| `src/record.js` | Records the real app's `fetch` calls for the current sampled request | `capture.js`, `keys.js` |
| `src/index.js` | The public API: `createTwin()` returns `wrap`, `stats`, `differences`, `idle` and `close` | all of the above |
| `src/ghost.js` | Starts one ghost thread with no app preloads; asks it, stops it | `worker.js` |
| `src/route.js` | Names a request's route: the path with ids replaced | — |
| `src/identity.js` | Names a version by its code: sha256 over its package's code files | `node:crypto` |
| `src/evidence.js` | Counts per segment and route for one candidate, in a checksummed file written whole | `node:fs` |
| `src/decide.js` | The switch-on decision: rule of three, segments, `maxUncovered`, `minHours` | — |
| `src/twin-thread.js` | The twin's own thread: coordinator, evidence, decision, switch requests | `coordinator.js`, `evidence.js`, `decide.js` |
| `src/router.js` | Sends each request to the version serving; the control file | `capture.js` |
| `bin/kern-twin.js` | `status`, `serve`, `build`, `test`, `feedback` and `comment` | `router.js`, `evidence.js`, `decide.js`, `build.js`, `cases.js`, `feedback.js` |
| `src/manifest.js` | Reads `kern twin plan`'s manifest, checks it against the code, attributes differences, matches `accept` | `node:crypto` |
| `src/coverage.js` | In a ghost: which characters of each changed line ran, directly or through a source map | `node:inspector/promises` |
| `src/twin.rs` (Rust) | `kern twin plan`: changed lines of code with their commit and pull request | `drive.rs`, git |
| `src/ghost-core.js` | The ghost body shared by threads and processes | `world.js`, `ghost-io.js`, `coverage.js`, `adapters/` |
| `src/ghost-common.js` | What thread and process ghosts share: their environment, asking a request, answering a real read | — |
| `src/process-ghost.js` | A ghost in its own Node process, under the permission model or in `bwrap` | `node:child_process` |
| `src/adapters/*.js` | Record a client library in the app's thread, replay it in the ghosts: `http`, `pg`, `ioredis` | `keys.js`, `capture.js` |
| `src/real-reads.js` | Allow-listed reads made by the twin's thread, once per request, for both ghosts: HTTP (`realReads`) and read-only Postgres (`pgReads`) | `record.js`, `adapters/pg.js` |
| `src/turn.js` | Whose turn it is inside a ghost: only the current request's (or the start's) timers and calls run; others wait forever | `node:async_hooks` |
| `src/overlay.js` | The in-memory file overlay | `node:fs` |
| `src/build.js` | `kern-twin build`: the old version beside the new one, with its packages | git, the package manager |
| `src/scrub.js` | Stand-ins for private values, of the same shape, consistent within a case | `node:crypto`, `keys.js` |
| `src/cases.js` | Cases on disk, written whole, read back exactly | `node:fs` |
| `src/replay.js` | One version on one case, in a fresh sealed ghost | `ghost.js` |
| `src/case-test.js` | The cases as `node:test` tests, for `kern-twin test` | `cases.js`, `replay.js`, `compare.js` |
| `src/feedback.js` | Who fixes a difference; `kern-feedback/v1`, the agent's copy of it, and the pull request comment | — |
| `proof/kit/*` | The proof kit: the twin around a real app in Docker, scripted traffic, the numbers, and the report | — |

## 5. Decisions and why

1. **Compare whole requests, not single functions.**
   - Requests and responses are plain data.
   - Any framework on `node:http` works: Express, Fastify, Koa, Hono on Node.
   - A request is exactly what a customer experiences.
   - Function-level twins were rejected. Arguments are often live objects that cannot be copied to another thread, and functions share module state.
   - Finer attribution comes in Phase 3 from coverage, not from a smaller unit.
2. **Two ghosts in one sealed world; reality only checks faithfulness.** Comparing the new ghost with the real run would drown in noise: real time, real randomness, real cache state. Both ghosts share one deterministic world, so noise disappears by construction. Today's replay tools need hand-written filters instead; Curtail's own example needed 12 rules to turn 3,730 differences into 2. Reality is used for one question: did the old ghost reproduce what really happened?
3. **Worker threads for isolation.**
   - Each ghost has its own heap and module graph, so in-memory state cannot leak between ghosts or into the app.
   - `resourceLimits` caps memory.
   - `terminate()` stops even a busy loop.
   - Same-thread ghosts were rejected because of shared state. Child processes are heavier; they remain the option for native crashes later.
4. **Replay through the real HTTP stack on loopback.** Frameworks depend on many details of `IncomingMessage` and `ServerResponse`. A real local server and client is the most faithful replay, and the simplest to get right. Fake request and response objects were rejected.
5. **Deny I/O by default; add replayable clients one by one.**
   - Phase 1 replays `fetch`.
   - Any other way out is refused. When both versions reach for it, the request is "not compared": honest, never silently wrong.
   - When only the new code reaches for it, on the same request in the same world, that reach is itself a difference (kind `refused`). Left "not compared", a rare path that starts a program or opens a connection could switch on unseen, under `maxUncovered`.
   - `pg`, `node:http` clients and Redis come in Phase 4.
6. **No dependencies, Node 22.3 or newer, ESM.** Nothing to audit in the supply chain, and the twin runs inside customers' production. (The seal needs `process.getBuiltinModule`, from Node 22.3; on an older Node the twin stays off and says so.)
7. **One request at a time per ghost pair; restart both together.** Both ghosts then always have identical histories, so a difference can only come from the code. Throughput is managed by sampling and, later, by more pairs.
8. **Fail open for customers, fail closed for comparisons.** Any doubt about a comparison means "not compared". Any internal error switches the twin off and the app carries on.
9. **The Rust `kern` tool stays the build-time analyser.** It already knows changed lines, measured lines and git history (`drive.rs`), and it is the PR-level product (`kern drive`). The runtime is JavaScript because it runs inside customers' Node processes.
10. **Judging in the twin's own thread (Phase 2).**
    - Comparing two large JSON bodies can take tens of milliseconds. On the app's thread, every customer would wait for it.
    - The app's thread copies and hands over; one worker judges, counts and decides.
    - Bytes cross as exact copies. A pooled Buffer posted to a thread carries Node's whole 8 KiB pool along, other requests' data included (measured).
11. **Evidence is one small file, written whole with a checksum.**
    - Append-only lines would grow without bound and need compaction.
    - A whole file written through a temporary file and a rename is never half-written. Its sha256 makes a hand edit or a damaged file start again, never count.
    - The evidence belongs to one candidate, named by its code, so trust never carries over to code it was not earned on.
12. **Both versions in the app's process (Phase 2).**
    - Switching inside the process is instant: the next request goes to the other version, and switching back needs no deploy.
    - The cost: once the new version serves, two copies of the app live in one process. So `createApp` must start no background work and must load twice safely.
    - Apps that cannot do that use `wrap()` and switch with their deploy system, reading `kern-twin status`.
13. **A switch back holds switching on by itself.** A person, or the twin itself, decided the old version should serve; the twin does not overrule that until a person switches the new version on again.
14. **Changed lines are judged per character, across requests (Phase 3).** One request takes one branch, so a line with a ternary never runs in full in one request. Per code character the evidence counts the clean comparisons that ran it, and a line is proven when every character has run often enough. This keeps kern drive's rule (nothing left out) while adding runs up the way a ghost sees them.
15. **The old side counts.** The lines a change removed or replaced must have run too: a branch only the old code had (the blank-field case) holds switch-on until real traffic has taken it.
16. **Calls are matched by shape when their payload carries the clock (Phase 4).** A ghost's clock is frozen at the request's arrival, so an insert stamped with the time can never match the real run's exactly. The shape (method and address, query text, command and key) still does. Writes are checked against reality by shape and between the ghosts exactly.
17. **`node:http` is replayed through a real local server,** so every client library built on it works unchanged.
18. **Isolation in layers.**
    - Thread ghosts are sealed in JavaScript: cheap, and enough for ordinary app code.
    - Process ghosts add Node's permission model, which stops code that ran before the seal.
    - `bwrap` adds the kernel's: no network, and a read-only file system.
19. **Real reads are made by the twin's thread, never by a ghost,** once per request, so both ghosts see one world.
20. **A case is written only if it still shows the difference after scrubbing (Phase 5).** Otherwise the difference depends on a private value, and a person, not an agent, must look.
21. **Stand-ins are keyed per case by a secret that is thrown away:** consistent inside a case, unlinkable across cases, and shaped like the value so the app's own checks still pass. A UUID stays a valid UUID, a hex value stays hex, and an address or a form is scrubbed piece by piece, every piece that did not change kept exactly as written. Every search takes time in proportion to the text.
22. **A known difference is never forgotten, and a fix earns its evidence again.** Every later candidate runs every case before it may switch on. Evidence never carries over from one candidate to another:
    - clean comparisons of requests a fix did not touch say nothing about the requests it did touch, which is where it most likely fails;
    - a request that ran none of the fixed code can still behave differently, through state that code left behind.
23. **What an agent reads never says how to allow a difference.** As in Kern's `for_agent`, the agent's copy of the verdict (the comment, `kern-twin feedback`) leaves out the allow hints, so an agent fixes its change instead of allowing it. A person adds allow rules.
24. **An app's start is recorded, not stubbed (Phase 6).** Real apps read while they start, and a ghost has no network. The old version's start in the app's own process is recorded like a request, and the ghosts start from it. A start that asks for something the record cannot answer is named, call by call.
25. **In a ghost, only the current turn gets time (Phase 6).** Timers and calls that an earlier turn left running wait forever. Background work (a queue's polling loop, a cron tick, a retry) can then neither mix into another request nor fail loudly.
26. **Transaction control is a session call (Phase 6).** `BEGIN`, `COMMIT` and `ROLLBACK` carry no data, so a new code path may open a transaction the recording never began; they are answered from any recorded one and never used up, while the writes inside stay used-once and compared. Likewise the calls a client library makes about the connection or the server (a version check, `SET`, `SHOW`).
27. **A read the new code makes that the old never did can be answered for real, read-only (Phase 6, opt-in).** With `pgReads`, the twin's own thread (never a sealed ghost) makes such a Postgres read inside `BEGIN TRANSACTION READ ONLY`, once for both ghosts, so a request on a changed path can be compared instead of left uncompared. It is off by default; a write is refused by the query text and by the database itself.
28. **The proof counts what adopting the twin took (Phase 6).** Every line of glue an app needed is in the report, beside what the twin found and what it could not compare.

## 6. The maths

- **The bound.** After n clean comparisons with no unexplained difference, the true rate of unexplained differences is below 3/n with 95% confidence (the rule of three, from −ln 0.05 ≈ 3).
- **Time to trust a path:** T = 3 / (ε × r), where:
  - ε is the allowed rate;
  - r is the number of compared requests per second that take the path, which is the request rate × the sample rate × the path's share of traffic.

  Example: ε = 1/10,000 and r = 1 request per second give 30,000 s, about 8.3 hours.
- **Throughput.** One pair compares about 1 / (t_old + t_new + overhead) requests per second. The sample rate should keep the sampled rate under that; beyond it, the queue sheds load and counts it.
- **Traffic changes over the day.** The bound assumes typical requests. High-stakes switch-on also requires a minimum time that covers a full daily cycle (Phase 2 option `minHours`).
- **Per customer group.** The bound is kept per segment (for example a tenant or a plan). Switch-on can require it for each listed segment.

## 7. Modes (built in Phase 2)

- **Shadow.** The old version serves, and the twin compares.
- **Active.** The new version serves.
  - The same two ghosts keep running.
  - Faithfulness is now checked against the new version's ghost, because the new version served the real request.
  - The machinery is the same, with roles swapped.
- **Switching on.** The twin's thread asks for it when `switchOn` is `'auto'` and `decide()` is ready, unless switching on is held. A person can switch with `twin.serve()`, `kern-twin serve` or the control file.
- **Switching back.**
  - An unexplained difference while the new version serves switches serving back to the old version before the next request, and holds switching on by itself.
  - The control file keeps the state across restarts. If it cannot be read, the router keeps its last known value; if there is none, the old version serves.
  - The router reads the control file in the same queue as its switches. A read of older content that was in flight during a switch back can then never switch the new version on again.
  - If the twin's thread fails, the old version serves.

## 8. Failure modes

| Part | Failure | Effect on customers | How it is noticed | What happens |
|---|---|---|---|---|
| Capture | Throws | None: the request passes through | `onError` | Twin switches off with the reason |
| Recorder | Cannot copy a call or body | None | `record.problem` | That request is skipped and counted |
| Ghost start | A version does not load | None | `spawn` rejects | Twin off, reason names the version and the error |
| Ghost run | Uncaught error | None | `dirty` message or exit | Pair restarts; reported as a crash difference |
| Ghost run | Hangs | None | Timeout | Both terminated and restarted; reported as a hang |
| Ghost run | Out of memory | None | Worker exit | Pair restarts |
| Queue | Full | None | `dropped` count | Load is shed |
| Restarts | Too many | None | `restarts > maxRestarts` | Twin off |
| Log file | Cannot be written | None | `logErrors` | Differences kept in memory |
| Whole twin | Must stop now | None | `KERN_TWIN=off` | Never starts |
| Capture | The client aborts the request | None | `close` without `finish` | Counted as skipped |
| Twin thread | Crashes, or stops | None | Worker `error` or `exit` | Twin off with the reason; if the new version was serving, the old one serves |
| Evidence | Cannot be written | None | `evidenceErrors` | Kept in memory and written at the next try |
| Evidence | Damaged, or for other code | None | Checksum or candidate mismatch | Starts again; `evidence().note` says why |
| Control file | Cannot be read | None | `readControl` answers null | The last known value stays |
| Control file | Cannot be written | None | `writeErrors` | Serving changes anyway; the write is tried again every second until it lands, never over a person's newer edit; the file's older text is never taken for a person's switch |
| Router | The new version cannot load in the app's thread | None | `switchTo` answers false | The old version keeps serving; `reason` says why |

## 9. Budgets

- **Main thread, requests not sampled:** one random number and one function call.
- **Main thread, sampled requests:**
  - copying the bodies, at most `maxBodyBytes` each way;
  - copying fetch answers, also capped;
  - after the answer: one `setImmediate`, then one `postMessage` of exact bytes to the twin's thread.
- **The twin's thread:** at most 512 MiB of heap; all comparing, counting and deciding.
- **Ghosts:** two threads, each capped at `ghostMemoryMb`, one request at a time.
- **Memory held:** at most `queueLimit` × (two bodies plus recordings).
- **Measured cost:** recorded by Phase 1 Task 9 in `twin/README.md`, measured, not guessed.

## 10. What it does not do yet

- **Server types.** Only `node:http` servers. No HTTP/2. Streamed answers longer than the cap (server-sent events, downloads) are skipped.
- **Outgoing calls.** `fetch`, `node:http`/`https` clients, `pg` and `ioredis` are replayed. Other clients (MySQL, MongoDB, Redis Cluster, undici's own client) are refused in a ghost, so a request that uses one is not compared.
- **In-memory state.** Caches and counters inside the app differ between the real app and the ghosts, so such requests may be "not compared". Allow-listed real reads (`realReads`) let a cold ghost make the reads a warm real app skipped.
- **File writes.** A ghost may not write files; with `overlay: true` its writes land in memory for the request, are written down, and are read back by the ghost. Writes the overlay does not cover (symlinks, permissions, opening a file to write) are still refused.
- **Native add-ons.** A thread ghost may not load them, except packages listed in `addons` (pure computation, such as `bcrypt`), which act below these controls. Process ghosts add Node's permission model, and `bwrap` the kernel's isolation.
- **Frozen time.** Code that waits for time to pass in a busy loop never finishes in a ghost. That ends as a timeout, never as a wrong "same".
- **Coverage.** It compares the requests that happen. A path no request takes needs Phase 3's path list and Level 1's generated inputs (`kern drive`).
- **Scrubbing.** A body the scrubber cannot read (binary, a file upload) is kept and named in the case's `opaque` list. In other text (XML, say) only the patterns apply, not the private names. A value the app adds from its own configuration is not a request value, and is not scrubbed.

## 11. Reused from Kern

- `src/probe.cjs`: the seeded random generator, the fixed clock and the uuid builder, ported into `world.js`.
- `src/compare.rs`: the kinds of difference (new crash, now accepted, changed error) and refusals that turned into success, ported into `compare.js`.
- `src/monitor.cjs`: the list of effects worth watching.
- `src/drive.rs`: changed lines, measured lines and exported functions (Phase 3).
- `src/feedback.rs`: `kern-feedback/v1` instructions for agents (Phase 5).
- `src/policy.rs`: the version 5 `delta`, stating the intended change (Phases 2 and 5).
- `src/attest.rs`: signing the switch-on evidence for auditors (after Phase 6).

## 12. Phases

Each phase ends with working, tested software.

1. **Shadow twin core** (planned).

   Accepted when:
   - every test in `twin/test/` passes in the pinned Node 22 image on Linux and with Node 22 on Windows;
   - the five customer scenarios are caught (crash on blank data, rounding, open door, double charge, renamed field) with the customer's answer untouched;
   - time and randomness never show up as differences;
   - every fail-safe test passes;
   - the cost is measured and written in `twin/README.md`.
2. **Evidence and switch-on** (built). Its acceptance tests are in `twin/test/switch.test.js`, `evidence.test.js` and `decide.test.js`.
   - Builds: counters per route and segment, the rule-of-three bound, `minHours`, the control state and router, active mode with roles swapped, instant switch-back, and counters kept on disk in append-only JSON lines with a checksum.
   - Accepted when:
     - simulated traffic switches on exactly when n ≥ 3/ε;
     - a seeded regression in active mode is switched back within one request;
     - counters survive a restart.
3. **Changed paths and named causes** (built). Its acceptance tests are in `twin/test/paths.test.js`; the Rust half is in `tests/twin.rs` and `twin/scripts/e2e-plan.mjs`.
   - Builds:
     - `kern twin plan <repo> <base> <head> <out.json>` (Rust, reusing `drive.rs` and git blame), which writes a manifest of changed lines with their pull requests;
     - per-request V8 precise coverage in both ghosts (`Profiler.takePreciseCoverage`), mapped through source maps to changed lines;
     - evidence per path, keyed by a hash of the code;
     - a list of changed paths no request has run;
     - each difference attributed to a pull request.
   - Accepted when:
     - the blank-field path is listed as never run before switch-on;
     - a candidate with three pull requests names the right one for each difference.
4. **Real stacks** (built). Its acceptance run is `twin/examples/shop/run.sh`; the adapters are tested against real servers by `twin/scripts/test-stacks.sh`.
   - Builds: `kern-twin build --repo <dir> --base <sha> --out <folder>` (builds the old version with its own `node_modules` when dependencies changed), adapters for `pg`, `node:http`/`https` clients and `ioredis`, allow-listed real reads, and an in-memory file overlay.
   - Accepted when an Express and Postgres sample app (docker compose) compares at least 95% of sampled requests.
5. **Fix loop** (built). Its acceptance test is `twin/test/fixloop.test.js`.
   - Builds:
     - each difference becomes a replayable case (the request, the recording and the old answer) that runs as a `node:test` file;
     - `kern-feedback/v1` instructions;
     - a pull-request comment;
     - scrubbing of private values;
     - a known difference is never forgotten: every later candidate runs every case before it may switch on, and a fix earns its evidence again (carrying evidence over was rejected as unsound; see the Phase 5 plan, decision 6).
   - Accepted when a seeded bug becomes a case, a scripted fix makes the case pass, and the candidate switches on.
6. **Proof on real apps** (built). Its report is `reports/twin-proof-<date>.md`; the proof kit is `twin/proof/`. Built on Infisical #6995 (the twin caught the change, zero false alarms) and firecrawl #2638 (a log-only change: the twin stayed silent, every changed line proven).
   - Builds: runs on 2–3 open-source Node apps with real pull-request pairs from the 23-PR set (`C:\ke`), driven by their end-to-end tests or recorded traffic.
   - Publishes: the cost, the share compared, the differences found and the false alarms.

## 13. The quality bar

- **Every rule in section 3 has a test that tries to break it.**
- **Every part in section 8 has a fault test.**
- **Determinism is tested, not assumed.** The same request gives identical results from both ghosts, including time, randomness and uuids.
- **Nothing passes silently.** Every request is counted as same, different, not compared, skipped or dropped.
- **No dependencies.**
- **Tested Linux first** in the pinned image `docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392`, plus Windows with Node 22.
- **The cost is measured and published, never guessed.**

## 14. How to test

From the repository root.

On Windows with Node 22 (quick loop):

```powershell
cd twin; node --test --test-concurrency=1 "test/**/*.test.js"
```

On Linux (WSL, from `/mnt/c/Users/krish/Downloads/Kern`). WSL's own Node is 18, which is too old, so the tests run in the pinned image:

```bash
docker run --rm -v "$PWD/twin:/twin" -w /twin docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392 npm test
```
