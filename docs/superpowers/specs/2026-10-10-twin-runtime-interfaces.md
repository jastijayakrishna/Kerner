# Twin runtime: the contracts between phases

Date: 2026-10-10.

This file pins what each phase hands to the next: file names, function shapes, message shapes, option names and file formats. Every phase plan keeps to it. A plan that has to change a contract changes this file in the same commit and says why.

Phase 1 is built, on branch `twin`. Its code is the starting point. Where this file and the Phase 1 code differ, this file describes the state after the phase named in each row.

## 0. Facts measured before planning (2026-10-10)

Measured on Node 22.12 (Windows) and on Node 22.23.3 in the pinned Linux image. The probe scripts are in the planning session's scratchpad.

| Fact | Consequence |
|---|---|
| A worker started with `execArgv: []` still runs a `--require` named in `NODE_OPTIONS`. With `execArgv: []` and an `env` without `NODE_OPTIONS`, it does not. | Ghosts start with both (Phase 2). |
| Posting a small `Buffer` to a thread copies Node's whole 8 KiB buffer pool, which held another request's data in the test. `new Uint8Array(buffer)` copies only its own bytes. | Every byte field posted between threads is an exact copy (Phase 2). Phase 1 was safe only because it posted base64 text. |
| Inside a worker, `process.kill` reaches the whole process. | Ghosts refuse it (Phase 2). |
| An aborted request emits `close` with `writableFinished` false and never `finish`. | It is counted as skipped (Phase 2). |
| A request's `data` and `end` events run outside the `AsyncLocalStorage` context the listener ran in, even for a body that came in the header's packet. Re-entering the context in `req.emit` and `res.emit` keeps it, across body events, timers, outgoing calls and keep-alive. | Every event of a sampled request and its answer runs inside its record (Phase 2). Without it, body parsers such as `express.json` lose every call made after the body was read. |
| `inspector/promises` works inside a worker. `Profiler.takePreciseCoverage` resets the counts each time. A ternary branch never taken is reported as a range with count 0 that starts at its `?` or `:`. `Debugger.getScriptSource` returns the source V8 ran. | Per-request coverage of changed lines (Phase 3). |
| The permission model refuses file writes, programs, threads, native add-ons and `process.binding`. It does not refuse network connections. The flag is `--permission` on Node 22.23 and `--experimental-permission` on Node 22.12. | Process ghosts (Phase 4). The network stays sealed in JavaScript, plus `bwrap` where it runs. |
| `bwrap --unshare-user --unshare-net` gives a process no network (`ENETUNREACH`). Inside Docker this works only as a non-root user, with seccomp and AppArmor unconfined. Node's IPC channel works through it. | The `bwrap` sandbox and its test script (Phase 4). |
| A worker accepts `--import` in `execArgv`. A child process under the permission model talks over IPC with `serialization: 'advanced'`, and bytes arrive as `Uint8Array`. | Process ghosts use the IPC channel; preloads a ghost needs come in through `ghostExecArgv` (Phase 4). |
| The permission model refuses the inspector (`ERR_ACCESS_DENIED`). | Measuring changed lines (Phase 3) works in thread ghosts, and in process ghosts only inside `bwrap` without the permission model (Phase 4). |
| One request takes one branch, so a line with a ternary never runs in full in a single request. | Changed lines are judged per code character, added up over clean comparisons (Phase 3). |
| In pg 8.23.1 every query, `Pool.query` included, goes through `Client.prototype.query`. In ioredis 6.0.0 every command, pipelines and transactions included, goes through `Redis.prototype.sendCommand`. | The `pg` and `ioredis` adapters patch those two points (Phase 4). |

## 1. Threads and processes

| Where | What runs there | Since |
|---|---|---|
| The app's own thread (main) | The real app, plus four small pieces: `capture.js` (copying), `record.js` (recording calls), `router.js` (which version serves) and a thin `index.js` facade. No comparing, no counting and no file writes happen here | Phase 2 |
| The twin thread (one worker) | `twin-thread.js`: the coordinator, the judging, the evidence and the switch-on decision. It starts the ghosts, talks to them, and makes their allow-listed real reads (Phase 4) | Phase 2 |
| Thread ghosts (two workers, started by the twin thread) | `worker.js`: one version of the app in a sealed world, sealed in JavaScript. From Phase 4 it is a thin transport around `ghost-core.js` | Phase 1 |
| Process ghosts (two child processes, started by the twin thread) | `process-ghost-main.js`: the same ghost core in its own Node process. Node's permission model is on, or it runs inside `bwrap` with no network | Phase 4 |

The twin thread is `unref()`ed: it never keeps the app's process alive by itself. Only `idle()` holds it, while someone waits for it; otherwise an app that closed its server and then awaited `idle()` would end with the work half done (found while building Phase 2).

**Turns inside a ghost (Phase 6, `turn.js`).** The app's start is one turn (`'start'`), and each request is another (its job number); the turn is carried by `AsyncLocalStorage`.
- Only the current turn's timers fire, and only its calls are answered and counted.
- A timer an earlier turn set never fires. A call made outside the current turn waits forever and is written down nowhere.
- The ghost's harness keeps a request's turn through the request's body events.
- A turn ends one tick after its answer, as a request's record does in the app's thread.

## 2. Messages

**Binary fields.**
- Every byte field travels as an exact-size `Uint8Array`, made with `new Uint8Array(buffer)`.
- It is never a pooled `Buffer` (see section 0), and never base64.
- Cases on disk (Phase 5) write bytes as `{ "$bytes": "<base64>" }` and dates as `{ "$date": "<ISO>" }`, so a case reads back exactly.

**Main → twin thread:**
- `{ type: 'capture', capture }`: one served, sampled request. Its shape is in section 3.
- `{ type: 'skip', reason, route, segment, serving, start }`: a sampled request that could not be copied, or that was aborted.
- `{ type: 'serving', side, reason, hold, router }`: serving changed, a router now exists (`router: true`), or a switch did not happen (the candidate could not load in the app's thread), so the twin thread, which may have asked for it, stays in step with the router. `side` is `'old'` or `'candidate'`; `hold` is true after a switch back or a failed switch on, so the twin thread does not switch on by itself.
- `{ type: 'load', load: { start, seed, io } }` (Phase 6, `load: 'record'` only): the record of the old version's start in the app's thread. It is sent once, and the ghosts start only then. `wrap()` sends one with no calls.
- `{ type: 'close' }`.

**Twin thread → main:**
- Exactly one of these, at start:
  - `{ type: 'ready', candidateId, snapshot }`: the first snapshot comes with it, so `evidence()` works as soon as `createTwin` resolves;
  - `{ type: 'disabled', reason }`.
- `{ type: 'snapshot', snapshot, judged }`:
  - sent after the queue drains, and at most every 250 ms while it is busy;
  - `judged` counts every capture and skip handled so far;
  - main keeps the latest snapshot for `stats()`, `differences()`, `decision()` and `evidence()`;
  - from Phase 5, `snapshot.checking` is true until the known cases are checked and the switch the last check may allow has been asked for. Main's `idle()` waits for it, because a snapshot sent while a check ran could otherwise end `idle()` before that switch.
- `{ type: 'serve', side, reason }`: the twin switches serving itself. It does this for an earned switch-on, or to switch back after an unexplained difference.
- `{ type: 'closed', snapshot }`: the answer to `close`, after the evidence is written.

**Twin thread → ghost:**
```js
{ type: 'job', id, job: { start, seed, request: { method, url, headers, body: Uint8Array }, io: IoEntry[], coverage: boolean } }
{ type: 'answer', id, answer }   // Phase 4: the answer to a 'real-read': { status, headers, body } or { error }
```
`coverage` arrives in Phase 3.

**Ghost → twin thread (a result):**
```js
{ type: 'result', id, result: { status, headers, body: Uint8Array | null, error?, timedOut?, effects: string[], events: [{ type, detail }], lines? } }
```
- `lines` arrives in Phase 3, and only when `job.coverage` is true.
- Its shape is `{ [lineKey]: mask }`, for example `{ 'new:src/app.js:17': '111…10000' }`.
- A mask has one character per code character of the line: `1` ran, `0` did not.
- Only lines with at least one `1` are listed.

**Ghost → twin thread (other):**
- `{ type: 'dirty' }`: this ghost's state may be broken, so the pair must restart.
- `{ type: 'load-lines', lines }` (Phase 3): the keys of the changed lines that ran while the app loaded. It is sent before `ready`.
- `{ type: 'real-read', id, job, method, url, headers }` (Phase 4): a ghost asks for an allow-listed real read. The twin thread makes it once per request, and both ghosts get the same answer.

## 3. Shapes

**Capture** (main → twin thread):

```js
{
  start, seed,             // when the real request arrived (ms), and its random seed
  route,                   // 'GET /users/:id': options.route(req), else routeOf(method, url)
  segment,                 // options.segment(req), default 'all'; 'unknown' when that function throws
  serving,                 // 'old' or 'candidate': which version answered the customer
  request: { method, url, headers, body: Uint8Array },
  response: { status, headers, body: Uint8Array },
  io: IoEntry[],           // every recorded outgoing call
  effects: string[],       // the call keys of the writes among them
}
```

**IoEntry.** Every entry has `kind`, `key` and `write`.

| Kind | Fields | Since |
|---|---|---|
| `fetch` | `method, url, sent: Uint8Array, status, headers, body: Uint8Array \| null, error` | Phase 1 (base64 then); Phase 2 adds binary fields and `sent` |
| `http` | The same fields as `fetch`, for `node:http`/`https` client requests | Phase 4 |
| `pg` | `text, values, result: { command, rowCount, rows, fields }, error` | Phase 4 |
| `redis` | `command, args, reply, error` | Phase 4 |

- `key` is built by `keys.js`: `callKey(method, url, sent)` for `fetch` and `http`; from Phase 4, `pgKey(text, values)` and `redisKey(command, args)`. A `sent` body that is a JSON object or array of at most 256 KiB is named by its value, every object's keys in order (an object has no order, RFC 8259), so a refactor that builds the same body another way makes the same write.
- From Phase 4 every key is `<shape> <payload>`, and `shapeOf(key)` drops the payload. A ghost takes the unused entry with the exact key, else the next unused one with the same shape, and notes that match as `loose`. A payload that carries the clock or a random id can never match exactly; its shape still does.
- `sent` is the body the app sent. Phase 5 scrubs it and builds the key again from the scrubbed copy.

**Verdict** (inside the twin thread):

```js
{
  n, at, time,                        // capture number; ISO time; ms
  route, segment, serving, method, url,
  outcome: 'same' | 'stated' | 'different' | 'not-compared' | 'skipped',
  kinds: string[],
  details: [{ kind, path, old, new }],
  lines?: { [lineKey]: mask },                     // Phase 3: changed lines run, both sides, a mask per line
  pulls?: number[], cause?: number | null,         // Phase 3: pull requests whose lines ran; the cause when only one
}
```

Notes on the verdict:
- **`stated`:** the versions differ only where an `allow` rule says they may (Phase 2).
- **The kinds of `crash`, `hang`, `door-opened`, `effects` and `refused`:** only a rule that names that kind can state them. A path rule never does.
- **Something a ghost may not use** (a program, a connection no recording answers): reached for by the new code alone, it is `different` with kind `refused`, and nothing after it is compared. Reached for by both versions, or by the old one alone, the outcome is `not-compared` with kind `unsupported`, unless the writes already differ.
- **A read the recording cannot answer:** when only one ghost makes it, the outcome is `not-compared` with kind `unanswered`, not "different". Its answer is unknown, so nothing after it can be compared. A write one version makes and the other does not is still "different".
- **Neither ghost answered in time:** the outcome is `not-compared` with kind `no-answer`, unless the writes already differ. What either version would have answered is unknown, so a known case on which both versions hang is never counted as passed. Two ghosts that fail with the same error (the same uncaught error, say) are `same`: both versions ran the request and failed alike.
- **What goes to disk is scrubbed (Phase 5):** the evidence's examples and the differences log get the verdict with its URL and shown values scrubbed. Only `differences()`, in memory, keeps the real values. `routeOf` treats a path segment with an `@` inside as an id, so no email reaches the disk in a route name.

## 4. Files and their exports

| File | Exports | Phase |
|---|---|---|
| `src/keys.js` | `READS`, `digest`, `callKey`, `isWrite` | 1; Phase 4 adds `shapeOf`, `stable`, `pgKey`, `pgIsWrite`, `redisKey`, `redisIsWrite`, `readAllowed` and `isSession`; the 2026-10-12 hardening adds `plainAddress(url)` (an http or https address as fetch reads it, or null) and `jsonText(value, { sorted })` (JSON text at any depth, exactly as JSON.stringify writes it; with `sorted`, every object's keys in order) |
| `src/serialize.js` | `decodeBody`, `readBody`, `preview`, `diffValues` | 1; Phase 2 adds `exactBytes`; Phase 4 adds `revive`; it shares `jsonText` from `keys.js`. `diffValues`, `revive` and `jsonText` walk values with stacks of their own, so any depth JSON.parse reads is handled |
| `src/compare.js` | `VOLATILE_HEADERS`, `compareRuns(a, b)`, `faithful(real, ghost, who)`, `explain(verdict, route, allow)` | 1; `explain` and `who` in 2 |
| `src/world.js` | `installWorld`, `resetWorld` | 1 |
| `src/ghost-io.js` | `installGhostIO({ ports, addons, realReads, ask, overlay })`, `beginJob(io)`, `endJob()`, `take(key)`, `wrote(effect)`, `note(type, detail)` | 1; Phase 2 seals more and exports `take`, `wrote` and `note`; Phase 4 matches by shape and adds `ports`, `realReads`, `ask` and `overlay` |
| `src/ghost-core.js` | `runGhost(side, transport)`: one ghost, whatever carries its messages | 4 |
| `src/worker.js` | The thread ghost's entry | 1; a thin transport around `ghost-core.js` from 4 |
| `src/ghost-common.js` | `ghostEnv()`, `ask(ghost, post, id, job, timeoutMs)`, `answer(post, message, settings)` | 4 (`ghostEnv` moves here from `ghost.js`, which re-exports it) |
| `src/ghost.js` | `ghostEnv()`, `startGhost(side, settings)` → `{ kind, ask(id, job, timeoutMs), terminate(), onDirty(fn), loadLines }` | 2; `loadLines` in 3; `settings.kind` `'process'` in 4 |
| `src/process-ghost.js`, `src/process-ghost-main.js` | `ghostCommand(settings)`, `startProcessGhost(side, settings)`, with the same shape as `startGhost` | 4 |
| `src/adapters/index.js` | `ADAPTERS`, `ADAPTER_NAMES` | 4 |
| `src/adapters/http.js`, `pg.js`, `ioredis.js` | Each exports `{ kind, record(api), prepare?(api), replay(api) }` by default: `record` runs in the app's thread, `prepare` in a ghost before its seal, `replay` in a ghost after it | 4 |
| `src/real-reads.js` | `realRead(request, patterns, maxBodyBytes)` → `{ status, headers, body }` or `{ error }` | 4; Phase 6 adds `pgReadable(text)` (one statement, a read, calling nothing that acts even read-only) and `pgReader(url, appModule)` |
| `src/overlay.js` | `installOverlay({ wrote })`, `resetOverlay()` | 4 |
| `src/build.js` | `buildOld({ repo, base, out, install, build })` | 4 |
| `src/identity.js` | `packageRoot(file)`, `versionId(file, exportName)` | 2 |
| `src/coordinator.js` | `class Coordinator`, which runs inside the twin thread (and directly in tests); `toRun(result)` | 1, reshaped in 2; Phase 5 exports `toRun` and adds `rerun(capture)` and `check(capture)`. A rerun takes its own changed-line counts and drops them, so the next real request is not credited with them |
| `src/route.js` | `routeOf(method, url)` | 2; Phase 5 makes a segment with an `@` inside an id |
| `src/evidence.js` | `canonical(value)`, `class Evidence` (section 6) | 2; Phase 3 adds lines |
| `src/decide.js` | `needed(epsilon)`, `totals(snapshot, segment)`, `decide(snapshot, policy, now)` | 2; Phase 3 adds `lineStatus`, `pathsOf` and the line rule; Phase 5 adds the known-case rule |
| `src/twin-thread.js` | The twin thread's entry | 2; Phase 5 writes cases, checks the known cases and reports `checking` |
| `src/router.js` | `readControl(file)`, `writeControl(file, value)`, `class Router` (section 5) | 2 |
| `src/capture.js` | `current`, `wrapListener(listener, hooks)` | 1, extended in 2; Phase 6 adds `loadRecord()` and `settleLoad(record)` |
| `src/turn.js` | `begin(id)`, `end()`, `within(id, fn)`, `owner()`, `isCurrent(id)`, `mine()`, `never()`, `keepTurn(emitter, id)`, `turnText(id)`, `turnOf(text)` | 6 |
| `src/record.js` | `readCapped`, `recordFetch(maxBodyBytes)` | 1 |
| `src/coverage.js` | `plausible`, `lineMasks`, `mappedMasks`, `decodeMappings`, `sourceMapOf`, `targetsOf`, `startCoverage`, `takeLines`; `isCode`, the rule for a code character, which `decide.js` reuses | 3 |
| `src/manifest.js` | `lineHash`, `loadManifest`, `checkManifest`, `coveragePlan`, `checkRoots`, `attribute`, `acceptMatcher` | 3 |
| `src/scrub.js` | `DEFAULT_SCRUB`, `scrubber(rules)`, `scrubCapture(capture, rules)` → `{ capture, opaque }` | 5 |
| `src/cases.js` | `caseName(entry)`, `writeCase(dir, entry)`, `readCase(file)`, `runOf(saved)`. `readCase` throws on a file that is not a case, and on a case that says it reproduces but lacks what running it again takes (section 9) | 5 |
| `src/replay.js` | `replayCase(caseValue, version, settings)`: one version on one case, in a fresh sealed ghost | 5 |
| `src/case-test.js` | The `node:test` file `kern-twin test` runs | 5 |
| `src/feedback.js` | `DEFAULT_HUMANS`, `needsHuman(verdict, humans)`, `SCHEMA`, `toFeedback(cases, options)`, `forAgent(feedback)`, `commentBody(cases, head, options)` | 5 |
| `bin/kern-twin.js` | The CLI: `status` and `serve` (2), `build` (4), `test`, `feedback` and `comment` (5) | 2–5 |

## 5. The router and the control file

**`class Router`** runs in the app's thread:
- `Router.open({ old, candidate, candidateId, stateDir, onSwitch })` loads the old version's listener in the app's thread. It loads the candidate's listener only when serving first switches to it.
- `onSwitch(side, reason, hold)` tells the twin's thread how serving stands, before the next request, after every switch and after a switch that did not happen. If it throws, the switch still stands and is still written to the control file.
- `listener()` returns one request listener. It sends each request to the version serving at that moment and writes that side into the sampled request's record.
- `serving` is `'old'` or `'candidate'`, and `reason` says why.
- `switchTo(side, reason)` applies from the next request on. Switches happen one at a time, and so do the router's reads of the control file: a read comes after every switch asked for before it, and its write. A candidate that fails to load leaves serving at old, with the reason, and holds switching on by itself until a person switches the candidate on: trying again would run its factory in the app's thread on every verdict. `onSwitch` is told, and the control file says why. The method answers whether the switch happened.
- `close()` stops watching the control file.

**The control file** is `<stateDir>/control.json`. It is written atomically: a temporary file, then a rename.

```json
{ "version": 1, "candidate": "<candidate id>", "serving": "old", "hold": false, "changedAt": "2026-10-10T12:00:00.000Z", "reason": "start" }
```

- **`hold`:** true after any switch back to the old version, and after a switch to the candidate that failed because it could not load in the app's thread. The twin then does not switch on by itself until a person switches the candidate on.
- **On start:** if the file names the same candidate and says `candidate`, the candidate serves. Otherwise the old version serves.
- **While running:** the router checks the file every second with `fs.watchFile`, which does not keep the process alive. A person can switch by editing the file, or with `kern-twin serve old|candidate --state <dir>`. The file is read in the router's switch queue, so a read of older content can never undo a switch back made while it was in flight. The watcher never leaves a promise to reject, which would end the app's process.
- **If the file cannot be read:** the last known value stays.
- **If the file cannot be written:** serving changes anyway, and the write is counted in `writeErrors`. It is tried again every second, in turn with switches, until it lands, so a restart never brings back a candidate that was switched back. It is never written over a file a person changed since the router last wrote or read it: that change is the watcher's to follow. The text the router itself last wrote or read is never taken for a person's switch, so the file's older text cannot undo a switch whose write failed. (`writeControl(file, value)` answers the text it wrote.)
- **If the twin fails:** when the twin's thread stops unexpectedly, the old version serves, and the file says why.
- **A normal close:** `twin.close()` switches nothing, so a restart keeps a candidate that earned serving.

## 6. Evidence and the switch-on decision

**`class Evidence`** runs in the twin thread:
- `Evidence.open({ file, candidate, old, hashes })` loads `file` only if it is intact (valid JSON, version 1, matching sha256), for the same candidate, and gathered against the same old version (`old`, the old version's id: `settings.old.id ?? versionId(old.module, old.export)`, worked out in the twin thread beside the candidate's). Otherwise it starts empty and says why in `note`. Evidence never carries over from one candidate to another, not even to a fix (Phase 5 plan, decision 6), nor from one old version to another: every comparison was the candidate against one old version, and against a hotfix deployed as old it proves nothing. A file written before the old version was recorded names none, and counts as another old version. Without `old` (as `kern-twin status` opens it), the file is read whatever old version it names.
- `add(verdict)` counts one verdict, including `skipped` ones.
- `setLoadLines(keys)` (Phase 3) records the changed lines that ran while the versions loaded.
- `snapshot()` returns a copy of the data.
- `flush()` writes the file atomically. The twin thread calls it every 5 s, before every switch, and on close. Flushes run one at a time, in the order they were asked for: one resolves only once every write asked for before it, and its own, are on disk. So the thread's last flush on close covers every verdict, even with a timed flush still writing.
- Write errors are counted in `errors` and retried.

The evidence file is `<stateDir>/evidence.json`:

```json
{
  "version": 1, "candidate": "<id>", "old": "<the old version's id>", "since": 1760000000000,
  "segments": { "all": { "GET /users/:id": { "same": 0, "stated": 0, "different": 0, "notCompared": 0, "skipped": 0, "first": 0, "last": 0 } } },
  "unexplained": [ { "n": 1, "at": "...", "route": "...", "segment": "...", "serving": "old", "kinds": [], "details": [] } ],
  "lines": { "new:src/quota.js:2": { "hash": "<sha16 of the line text>", "touched": 0, "runs": 0, "clean": 0, "cover": "3321" } },
  "loadLines": [ "new:src/quota.js:1" ],
  "sha256": "<hex of the canonical JSON of everything above>"
}
```

- `since` is when the first comparison (same, stated or different) was counted, and 0 until then; `minHours` counts from it. A request that was skipped or not compared never starts it, since it says nothing of the candidate. A file written before keeps the `since` it has.
- `lines` and `loadLines` arrive in Phase 3. Each `lines` entry counts:
  - `touched`: requests that ran any of the line;
  - `runs`: requests that ran all of it;
  - `clean`: those of them that were clean;
  - `cover`: one digit per code character, the clean comparisons that ran it, capped at 9.
- Segments are capped at 200 and routes at 500 per segment. Anything past that is counted under `(other segments)` and `(other routes)`.
- `unexplained` keeps the first 20, scrubbed from Phase 5 on.

**Known cases** (Phase 5) are not evidence. They are what the twin already knows to be wrong. The twin thread keeps them in memory for its run, and `twin.evidence().known` shows them:

```js
{ pending: 0, failing: ['<case file>'], unchecked: [], stubs: [], unreadable: [] }
```

- `pending`: cases not yet run on both ghosts.
- `failing`: cases that still show a difference.
- `unchecked`: cases the ghosts could no longer compare.
- `stubs`: differences only a person can judge, until that person removes the file.
- `unreadable`: case files that could not be read, including a case that says it reproduces but lacks its world, request, recording or answers. One broken file holds the candidate; it never switches the twin off.

**`decide(snapshot, policy, now)`** returns:

```js
{ ready, reasons: string[], needed, bound: { [segment]: number }, waiting?: string[] }
```

The policy is `{ epsilon, minHours, segments, maxUncovered, minLineRuns?, accepted?, lines?, known? }`. `accepted` is a function made by `acceptMatcher(accept)`. `lines` is the manifest's Map of changed lines (line key → `{ text, ... }`, as `loadManifest` answers it): the line rule needs each line's text. In `segments`, the name `'*'` means all traffic together. A candidate is ready only when all of these hold:

1. **No unexplained difference** since the candidate started.
2. **Enough clean comparisons in every listed segment.**
   - The count is `same + stated`, of this candidate's own comparisons.
   - It must reach `needed = ceil(3 / epsilon)`. This is the rule of three: with no failures in n tries, the true rate is below 3/n with 95% confidence.
3. **Little left uncompared in every listed segment.** At most `maxUncovered` of the requests are not compared or skipped, because the bound says nothing about requests it never saw.
4. **Enough time.** At least `minHours` have passed since the first comparison (`since`); skipped and not compared requests do not start the clock.
5. **Enough runs of every changed line (Phase 3).** Each measured changed line, on both sides, must be one of these:
   - `proven`: every code character ran in at least `minLineRuns` clean comparisons;
   - `at load`: it ran while the version loaded;
   - `accepted`: it is listed in `accept`;
   - `no code`: its text has no code character by `coverage.js`'s own rule (`isCode`: braces, brackets, parentheses, `;`, `,` and blanks are not code), such as a `{` alone on its line, which Prettier writes in arrays of objects. `kern twin plan` keeps such lines, coverage never reports them, and nothing on them can run.

   `waiting` lists the lines that are none of these. `pathsOf()` gives every line's status: `never run`, `partly run`, `run`, `proven`, `at load`, `accepted` or `no code`.
6. **Every known case passed (Phase 5).** None is pending, failing, a stub or unreadable. An `unchecked` case does not hold the candidate: live traffic must prove that path.

## 7. Options of `createTwin`

| Option | Default | Phase |
|---|---|---|
| `old`, `candidate`: `{ module, export, id?, root? }` | Required. `module` and `root` are paths (a relative one is read from the working folder) or file URLs; `createTwin` makes them absolute, because a ghost requires the version's packages relative to its module. `id` defaults to `versionId()`, the sha256 over the code, template and data files of the version's package: an explicit list of extensions (JavaScript and TypeScript, JSON, SQL, GraphQL, Prisma, protobuf, YAML, TOML, templates, components, `.node` and `.wasm`), so files written at run time (logs, uploads) never change it. It also covers every package linked into a `node_modules` folder the version can load from (its own, those inside it and inside linked packages, and those of every folder above it) whose real path is outside every `node_modules` folder: workspace packages and `file:` dependencies, named by where the link sits; a link that loops back ends there, and the 20,000-file cap counts them too. `root` is used from Phase 3 | 1; `id` in 2; `root` in 3 |
| `sampleRate`, `timeoutMs`, `startMs`, `queueLimit`, `maxBodyBytes`, `ghostMemoryMb`, `maxRestarts`, `log` | As in Phase 1 | 1 |
| `route(req)` | `routeOf(req.method, req.url)` | 2 |
| `segment(req)` | Answers `'all'` | 2 |
| `allow`: `[{ route, paths?, kinds? }]` | `[]` | 2 |
| `stateDir` | `null` (in memory only: no control or evidence file) | 2 |
| `epsilon` | `0.001` | 2 |
| `minHours` | `24` | 2 |
| `segments` | `['*']` | 2 |
| `maxUncovered` | `0.05` | 2 |
| `switchOn`: `'manual'` or `'auto'` | `'manual'` | 2 |
| `ghostExecArgv` | `[]` (for example `['--import', 'tsx']`; trusted, runs before the seal) | 2 |
| `addons` | `[]` (native add-ons a ghost may load, by node_modules package name, or by the file's stem when it loads from a path outside node_modules; pure computation only, such as `bcrypt`) | 2; file-name match in 6 |
| `manifest` | `null` (the file `kern twin plan` writes). With it, `old.root` and `candidate.root` are required | 3 |
| `minLineRuns` | `3` (1 to 9) | 3 |
| `accept` | `[]` (for example `['old:src/x.js:12', 'new:src/x.js:20-24']`) | 3 |
| `ghosts`: `'thread'` or `'process'` | `'thread'` | 4 |
| `sandbox`: `'none'` or `'bwrap'` | `'none'` (`'bwrap'` needs Linux and process ghosts) | 4 |
| `adapters`: a subset of `['fetch', 'http', 'pg', 'ioredis']` | `['fetch']` | 4 |
| `realReads`: `['GET https://api.example.com/*']` | `[]` (GET and HEAD only; a trailing `*` matches by prefix, inside the pattern's own origin, after `..` is resolved; a pattern is a whole http or https address) | 4 |
| `overlay` | `false` (when true, ghost file writes land in an in-memory overlay, are recorded, and are never done) | 4 |
| `cases` | `null` (a folder for scrubbed, re-checked cases; every later candidate must pass them) | 5 |
| `scrub`: `{ headers, keys }` | `DEFAULT_SCRUB`, plus these: more private header names, and more value names (JSON properties, form and query names) whose values are private. Emails, card numbers, UUIDs and long tokens are scrubbed anywhere | 5 |
| `humans`: `{ kinds, hosts }` | `{ kinds: ['door-opened'], hosts: [] }`: these kinds, and writes to these hosts, need a person | 5 |
| `load`: `'none'` or `'record'` | `'none'`. With `'record'`, the old version starts in the app's thread inside a record of its own, and the ghosts start from its answers. For apps that read their database or cache while they start | 6 |
| `pgReads` | `null` (a Postgres connection string). A read the new code makes that the old never did is made for real by the twin's thread, read-only, inside `BEGIN TRANSACTION READ ONLY`, once for both ghosts. Off by default; needs the `'pg'` adapter | 6 |

## 8. The twin object

`createTwin(options)` resolves once the twin's thread is ready or the twin has been switched off. It returns:

| Member | What it does |
|---|---|
| `listener()` | Async. Loads the old version in the app's thread and answers a routing listener that also records. From Phase 2 on, this is the normal way to run |
| `wrap(listener)` | The Phase 1 way: records around the app's own listener, and the old version always serves |
| `stats()` | Counters for this run: `sampled`, `skipped`, `dropped`, `same`, `stated`, `different`, `notCompared`, `restarts`, `logErrors`, `evidenceErrors`, `queued`, `disabled`, `reasons`, `serving`; `adapters` (Phase 4); `cases` and `caseErrors` (Phase 5); `hold` and `servingReason` (the router's hold and reason, from the 2026-10-12 hardening). Synchronous |
| `differences()` | The first 100 unexplained differences of this run, with real values; the `log` file holds them all, scrubbed from Phase 5 on. Synchronous |
| `decision()` | The latest `decide()` result, or null before the first snapshot. Synchronous |
| `evidence()` | The candidate's evidence across restarts: `{ candidate, since, note, totals: { [segment or '*']: counts }, unexplained }`, and `known` (Phase 5). Synchronous |
| `paths()` | Phase 3: every changed line of the manifest, as `{ key, side, path, line, text, pull, touched, runs, clean, status }`, at most 2,000. Synchronous |
| `serve(side, reason)` | Switches serving by hand. Answers whether it switched |
| `idle()` | Resolves once every capture handed over so far has been judged, the known cases have been checked (Phase 5), and every switch that caused has been applied. While it waits, it keeps the process alive |
| `close()` | Writes the evidence, then stops the twin's thread and the ghosts |

## 9. File formats from later phases

**The manifest** (Phase 3). `kern twin plan <repo> <base> <head> <out.json>` writes it. It refuses a shallow clone, where git blame gives every line to the commit at the shallow edge:

```json
{
  "version": 1, "base": "<sha>", "head": "<sha>",
  "files": [
    {
      "path": "src/quota.js", "status": "modified",
      "changed": { "old": [2], "new": [2] },
      "old": { "2": { "text": "  return policy.limits ? policy.limits.max : 100;", "commit": "<sha>" } },
      "new": { "2": { "text": "  return policy.limits.max;", "commit": "<sha>" } }
    }
  ],
  "commits": { "<sha>": { "summary": "Tidy the quota check (#102)", "pull": 102 } },
  "skipped": [ { "path": "src/big.js", "why": "generated or minified code" } ]
}
```

The fields:
- **`changed`** lists every changed line number of each side, code or not.
- **`old` and `new`** hold the measured changed lines, using the rules of `kern drive`'s `measured()`. Each has its text and the commit in `base..head` that changed it:
  - new lines: `git blame` at head;
  - old lines: `git blame --reverse`, then the next commit on the first-parent path.
- **`pull`** is the number in a trailing `(#123)` or in "Merge pull request #123", or null.
- **Line keys** are `<side>:<path>:<line>`, for example `old:src/quota.js:2`.

**The case** (Phase 5) is written to `<cases>/<time>-<n>-<route slug>.json`, whole, through a temporary file and a rename:

```json
{
  "version": 1, "candidate": "<id>", "n": 7, "time": 1760000000000, "route": "POST /policy", "segment": "all",
  "adapters": ["fetch"], "overlay": false,
  "kinds": ["crash"], "lines": {}, "pulls": [], "cause": null,
  "human": false, "scrubbed": true, "opaque": [], "reproduces": true,
  "world": { "start": 1760000000000, "seed": 42 },
  "request": { "method": "POST", "url": "/policy", "headers": {}, "body": { "$bytes": "<base64>" } },
  "io": [],
  "old": { "status": 200, "headers": {}, "body": { "$bytes": "<base64>" }, "effects": [], "events": [] },
  "new": { "status": 500, "headers": {}, "body": { "$bytes": "<base64>" }, "effects": [], "events": [], "error": "uncaught TypeError: ..." },
  "verdict": { "kinds": ["crash"], "details": [] }
}
```

- **Bytes** anywhere in a case are `{ "$bytes": "<base64>" }`, and dates are `{ "$date": "<ISO>" }`.
- **`adapters` and `overlay`** are the twin settings the case was recorded under; a replay uses them.
- **`opaque`** names bodies the scrubber could not read (binary, or broken compression). They are kept as they were, so a person must look before the case is shared.
- **`reproduces`** says whether the difference was seen again on the scrubbed copy. A case with `reproduces: true` must hold `world` (`start` and `seed`, numbers), `request` (`method`, `url`, a `headers` object and body bytes), `io` (a list) and both answers, `old` and `new`; `readCase` refuses one that does not.
- **A stub** is a case with `reproduces: false`. It has no `world`, `request`, `io`, `old`, `new` or `verdict`, only the route, the kinds and `why`. A person must look, and removes the file once done.
- **`human`** is true when a person must decide (section 7, `humans`).

**The agent's instructions** (Phase 5). `toFeedback(cases, options)` makes `kern-feedback/v1`, the shape of Kern's own `src/feedback.rs`:

```json
{
  "schema": "kern-feedback/v1", "status": "BLOCKED", "reason": "the twin found 1 unexplained difference on real requests",
  "agent_fixable": true,
  "violations": [
    { "rule": "twin-difference", "path": "src/app.js", "detail": "POST /limit: crash at result: status 200 → uncaught TypeError: ..., from #103",
      "instruction": "Run `npx kern-twin test --cases <dir> --candidate <module>`: ...", "allow": "allow: [{ route: 'POST /limit', kinds: [\"crash\"] }]" }
  ]
}
```

- **`agent_fixable`** is true only when every case reproduces and none needs a person.
- **The agent's copy,** `forAgent(feedback)` (Kern's `for_agent`), leaves out `reason` and every `allow`. `kern-twin feedback` prints it, and the comment carries it. An agent is never told how to allow its own difference.
- **The comment** (`kern-twin comment`) ends with `<!-- kern-twin:v1 head=<sha> -->`, never with Kern's `<!-- kern-feedback:v1 head=… -->`, so Kern's count of rejected attempts is unchanged. The twin prints it; posting it is a person's decision.
