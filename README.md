# Kern

**Admission control for AI-written code.** Any coding agent (OpenShell, Claude Code, Codex, Copilot, Devin) proposes a change. Kern decides whether that exact commit may be merged, and proves it with a signed in-toto attestation that a GitHub required check re-verifies.

Sandboxes such as OpenShell control what an agent can *do while it runs*. Kern controls what the agent's *output may become*:

- **Task contract.** The agent may change only the granted directories, within a change budget counted with a real line diff. `.github/`, `.env*`, lockfiles and other dependency or CI files are protected. A task may approve named packages (`"dependencies": { "manager": "npm", "lockfile": "package-lock.json", "allow": ["zod", "@types/*"] }`): the agent may then change `package.json` and the lockfile for exactly those packages and what they need, and nothing else in them.
- **Capability diff.** JS/TS/JSX are parsed with `oxc`. A patch is refused if it gives a module new authority, such as `child_process`, `fetch`, `process.env`, `eval`, dynamic `require`, or a new package. Unchanged authority never blocks an edit. A text, Markdown, CSS, HTML or `.d.ts` file that parses as JavaScript is checked the same way, since `require()` executes it, and loading a declaration file at runtime counts as new authority.
- **Independent verification.** A separate verifier runs the project's real test suite and operator-owned cases on the exact commit, each in a fresh gVisor sandbox with no network. Dependencies come only from the committed lockfile.
- **Behaviour diff.** The verifier runs the same checks on the base commit too and compares what the code actually did: network destinations, DNS names, programs started and secret-looking environment reads. A change that newly reaches out is blocked unless the task contract allows it, which catches a module that already had network or process access starting to use it somewhere new.
- **Proof of honest fix.** When the task contract asks for it, the verifier reads the test runner's own report on the exact commit: every test that ran before must still run and pass, none may be newly skipped, and for a bug fix a new test must fail on the old code and pass on the new code. An agent may rewrite or remove tests only while the old tests still pass on the new code and every bug they caught (Kern plants small bugs to find out) is still caught. Skipping tests, stopping them from running and changing the test runner's settings are refused, and a test that mocks the fixed code away does not count. Tests that already failed before the change, for example because they need the internet the sandbox does not have, are set aside: they must keep running, but they neither block the change nor count as proof.
- **Feedback for the agent.** Every block comes with a machine-readable verdict and plain instructions an agent can act on, never hints on how to loosen the gate. `kern run` hands it back to the agent and retries on its own, the PR check posts it as a comment, both with a retry cap, and every rejected attempt stays on record.
- **Merge audit.** `kern audit-merges` re-verifies the proof of every change merged in a period and lists the changes that skipped the gate, as JSON, HTML and CSV for auditors.
- **The old code is the rulebook.** A version 5 task states its change before the code exists. The verifier calls the changed code with the same inputs on the old and the new version and refuses anything that changed beyond the statement, any new crash, any file or export removed without saying so, new code that deletes files, and any route that now lets in someone it refused. Speed and paid API calls are measured on the way, removed code is explained by what it did and who added it, and dependency upgrades are proven to change nothing.
- **Signed proof at merge time.** The verifier emits an in-toto Statement with a SLSA verification-summary predicate. The `kern-verified` check re-verifies it against the PR head, so a later push invalidates it. With a hosted verifier, Kern verifies such a push again on its own and moves the branch to the newly signed commit.

What Kern guarantees: an agent's change cannot reach `main` unless it stays inside its contract, adds no authority the code did not already have, passes the real tests on the exact commit, and shows no new runtime reach, all proven by a signature only the verifier can make. What it does not guarantee: that the code is correct or well-intended. Code can be tailored to known tests, and authority a module already holds can still be misused. Kern narrows what an agent can change; review still decides whether the change is good.

```text
agent (any sandbox) ──commit──▶ kern ingest ──bundle──▶ kern verify (gVisor) ──in-toto──▶ kern promote ──▶ draft PR
                                                                                                     └▶ kern-verified required check
```

Kern is one static Rust binary. The same binary is the CLI, the verifier and the in-sandbox runner.

## Install

```sh
# Linux or macOS
curl -fsSL https://github.com/OWNER/kern/releases/latest/download/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://github.com/OWNER/kern/releases/latest/download/install.ps1 | iex
```

The installer picks the build for your machine (Linux x64 or ARM64, macOS Apple silicon or Intel, Windows x64, which Windows 11 on Arm runs as well), checks it against the release's `SHA256SUMS`, tries it, and only then puts it in place, so a failed install never leaves a broken `kern` behind. It installs to `~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\kern`, added to your PATH). Set `KERN_VERSION=v0.4.0` to install a particular release and `KERN_INSTALL_DIR` to choose the folder. Every release file also carries a GitHub build-provenance attestation: `gh attestation verify <file> --repo OWNER/kern`.

Nothing else is needed for setup, the merge gate or audits. Running an agent in Kern's own local box needs a container engine (below), and a verifier needs Linux with gVisor.

## Quick start (about 10 minutes)

```sh
# On the verifier: export its public settings once (operator template + public key)
kern profile operator.json /operator/keys/public.pem > profile.json

# In a repository: generate the task contract, operator config, verifier key, workflow and ruleset
kern init . profile.json OWNER/kern@COMMIT_SHA            # or: kern init . https://verifier.example
git add .github && git commit -m "Add the Kern merge gate"
gh api -X POST repos/OWNER/REPO/rulesets --input .github/kern/ruleset.json
```

`kern init` reads the committed tree and grants every top-level folder that holds code, docs or styles, and the root-level code, Markdown, HTML and CSS files. It leaves out dot folders (tool settings such as `.vscode/` or `.cursor/`), generated or vendored output (`dist/`, `build/`, `out/`, `coverage/`, `vendor/`), build and test configuration (`*.config.*`, `tsconfig*`), and the instructions coding agents read (`AGENTS.md`, `CLAUDE.md`, `GEMINI.md`), since an agent that rewrites those steers the next agent. CI files, lockfiles and manifests stay protected whatever the grants say. The change budget is 40 files and 4,000 changed lines, which fits 99% of 320 real agent pull requests. Every file it writes is validated by Kern's own parsers, and it refuses to overwrite an existing setup. The ruleset requires the `kern-verified` check from GitHub Actions only, so no other app can satisfy it, and it adds to existing branch protection rather than replacing it. Organizations can apply the same requirement to every repository with an org-level ruleset that requires the workflow.

## Proof of honest fix

AI agents sometimes make tests pass instead of making the code right: they delete or skip a failing test, stop tests from running, mock the code under test, or loosen an assertion. A version 4 task contract can ask Kern to rule that out:

```json
"tests": { "paths": ["test/"], "kind": "bugfix", "runner": "mocha", "command": ["/workspace/node_modules/.bin/mocha"], "edits": "checked" }
```

- **Kern reads the runner's own report.** `runner` is `mocha`, `jest`, `vitest` or `node`, and Kern adds the matching reporter to `command`: `--reporter json` for mocha; for jest `--ci` and a small results processor of Kern's own, because jest's `--json` also moves the tests' console output to stderr and changes what tests that capture output see; `--reporter=json --no-cache` for vitest; `--test-reporter=tap` right after `--test` for node. So Kern knows test by test what ran, passed, failed or was skipped. The command may not choose a reporter of its own, and vitest must run as `vitest run`. Exactly one report must come out of each run: output a test prints is ignored, and a second report is refused.
- **Before anything runs**, no change may touch the test runner's settings (`.mocharc*`, `jest.config*`, `vitest.config*`, `vite.config*`, `setupTests*`, Babel configuration and similar), new test files may not skip or focus tests (`.only`, `.skip`, `.todo`, `xit`/`fit` and the like, or `skip`/`only`/`todo` options), and code outside the tests may not use test-runner globals such as `describe` or `expect`, change an assertion library such as `node:assert`, or check whether tests are running (`JEST_WORKER_ID`, `VITEST` and similar). These checks run at every gate and give the agent early feedback. With `"edits": "locked"` existing test files may not change at all; by default (`"checked"`) they may, and the verifier checks the result as described below.
- **Then the verifier runs the suite in gVisor without network**, each run within the contract's time limit: the existing suite on the old code, everything on the new code, the old tests alone on the new code whenever the change adds or rewrites tests, and for `"kind": "bugfix"` everything on the old code with the new test files added. Each run gets its own writable in-memory copy of the repository, as a CI checkout would, with `CI=true`: tests can write logs and fixtures, but never the snapshot or the next run. The shared dependency install stays read-only, with empty in-memory `.cache`, `.vite` and `.vite-temp` folders for tool caches. A run that fails is run once more before it counts, and a test that fails only once is recorded as flaky. On the new code nothing may fail, every test that ran before must still run and pass, no test may be newly skipped, and the command itself must succeed. A bug fix also needs a new test that the runner reports *failing* on the old code: a test that already passes there does not show the bug, which also rules out a test that mocks the fixed code away, and a test file that cannot even load on the old code does not count. `"feature"` skips the third run, and `"refactor"` needs no new test.
- **The old tests also have to pass alone.** Without the new tests, the old tests, taken from the base commit, must pass on the new code too. Otherwise code could keep state that gives the new tests one answer and the old tests another, depending on which run first: returning a new result only on its first call passed both an old test and a new one that contradict each other, until this run was added.
- **Tests that already fail before the change are set aside.** Real suites often hold a few tests that cannot pass in an offline sandbox, or that are broken upstream. A test the runner reports failing on the old code, before the agent did anything, must still run on the new code, but it neither blocks the change nor counts as proof, and the evidence lists it (`already_failing`). The agent cannot influence that run. If no test passes on the old code, Kern still refuses: the suite has nothing to compare against.
- **Rewritten or removed tests are checked, not refused.** When a change edits or deletes existing tests, the verifier also runs the old tests, taken from the base commit, on the new code: they must still pass, so rewriting a test can never hide a change in behaviour. Then it plants small bugs in the code the old versions of those tests reach (a `>` becomes `>=`, a `+` becomes `-`, a condition is negated, a call is removed; at most one per line and 40 per change, spread from the top of each file to the bottom) and runs the old and the new tests against each one, with the bugs shared out over sandboxes that run side by side (as many as `KERN_VERIFY_PARALLEL` allows), each of which first checks that the tests pass without any bug. Every planted bug the old tests caught must still be caught by the new ones. A bug only counts as lost when both results repeat on a second run, so a flaky test never blocks. When some tests already failed before the change, the planting runs are judged by the runner's report rather than its exit code: a planted bug counts as caught only when a test fails that did not fail before. So a duplicate test can go and five weak tests can become one strong one, but the only test that catches a bug cannot just disappear, and the agent is told which planted bug its tests no longer catch.
- **A change of behaviour has to be the task.** With `"kind": "change"` the old tests may fail on the new code, for example when a feature is removed with its tests. Kern then lists every changed expectation in the evidence for the reviewer instead of refusing the change.
- Because the report shows the effect, it does not matter how a trick is written: an `.only` reached through an alias, a skip hidden in the code under test, a test renamed through a data file or an early `process.exit` all show up as tests that did not run, or as a run with no report.
- The result is a signed `honest-fix` check. Its evidence, `honest-fix.json`, records each run, how many existing tests still pass, every new test with its result on the old and the new code, and for rewritten tests the planted bugs: how many were checked, still caught, lost or flaky.

`kern init` writes this section with `"kind": "feature"` when it can tell which runner the repository uses; set `"kind": "bugfix"` for a bug-fix task. New tests must live in files the suite command runs. See [Evidence](#evidence-2026-10-08) for a run on a real project in gVisor.

## The old code is the rulebook (contract version 5)

Reviewing 300 lines an agent wrote is slow because nobody said what the change was supposed to do. A version 5 task contract says it before the code exists, and the old code is the rule for everything else:

```json
"delta": {
  "intent": "A format with a bare precision, such as %.f, uses precision 0; nothing else changes",
  "changes": [{ "function": "src/sprintf.js#sprintf", "when": "(fmt) => typeof fmt === 'string' && /\\.(?!\\d)/.test(fmt)",
                "examples": [{ "args": "['%.f', 2.4]", "returns": "'2'" }] }],
  "removes": [], "allow": []
}
```

The verifier calls the exported functions of the changed modules, and of the modules that import them, with the same arguments on the old and the new code, twice each, in gVisor with the network off, and compares what every call returned or threw, whole (a value too long to show is compared by a hash of all of it). The arguments come from the project's own tests (the old tests' calls first, every call written with plain values), from values nobody expects (`undefined`, `NaN`, `""`, `-0`, a 1,000-character string and the like), from the plain values in the module's old and then new source (so a value the change adds, such as a magic string, is tried too), and from seeded random mixes of these. Every function gets the same calls on both sides, the ones the task names first. Time and chance are the same on both sides (a fixed clock, seeded `Math.random` and `crypto` randomness), and a call that answers differently when the old code runs twice is set aside as noise. The code under test runs in worker threads; which calls the task states and whether its examples hold are decided outside them, and their messages count only when signed by the harness.

- **Inside the delta** (a listed function, where its `when` holds) behaviour may change, and every example must hold on the new code.
- **Outside it nothing may change.** A difference there is refused, and the agent is told which call changed. That includes an input the old code refused with an error that now gets an answer (what a dropped validation looks like) and an error of another kind (`RangeError` that became a `TypeError` deeper in); an error whose words alone changed is listed but not refused, since renaming a variable changes what the engine says. `"changes": []` states that nothing changes at all: a refactor, a feature behind a flag that is off, an upgrade.
- **`"delta": null`** states nothing: Kern still compares and lists every difference on the pull request, and refuses only what no change may do unstated.

Refused whatever the task states, unless its delta names it:

- **Never worse than before.** A call that returned on the old code and now throws, stops its thread or does not finish in time, on either run of the new code; calls the old code answered that the new code leaves unanswered (it stops early or becomes much slower); a module that no longer loads; an app that no longer starts. Every call runs with the network off, so code that no longer survives a failing network shows up here.
- **No silent destruction.** A deleted file, or an export that is gone or no longer a function, that `removes` does not list; new code that calls `fs.rmSync`, `unlink`, `rimraf` and the like (`"allow": ["delete"]` lets it); and any check, test or probe run that newly deletes or overwrites a file of the repository (`delete:` and `overwrite:` behaviours, which `behaviors` can allow by name).
- **No new open doors.** With an app recipe in the contract (`"app": { "setup": null, "start": ["/usr/local/bin/node", "server.js"], "port": 3000, "ready": "/healthz", "routes": ["GET /admin"], "roles": { "anonymous": {}, "user": { "Authorization": "Bearer test" } }, "env": {} }`), Kern starts the old and the new app in the sandbox and asks every route as every role. A route that refused a role (401, 403 or a redirect) or did not exist for it (404, 405) and now answers it with success, on either run, is refused (`"allow": ["access"]` lets it).

Measured from the same runs, shown on every comparison, and refused only when a delta states its change (unless `allow` names them):

- **Speed:** a function whose median call became at least three times and at least a millisecond slower.
- **Paid API calls:** calls such as `chat.completions.create`, `messages.create` or `generateText` that the change adds to a file, and attempts to reach OpenAI, Anthropic, Google, Twilio and other paid APIs, counted per host on the old and the new code.

**Why was this code here?** For code a change only deletes, Kern also runs the old code without just those lines and lists the calls that behave differently ("`add([undefined])`: returns 0 with it, returns NaN without it"). `kern ingest` and `kern task` add who last touched those lines, from the repository's history, and the merge check adds the pull request that brought them and the issues it closed, from GitHub.

**Safe upgrades.** For a repository with a lockfile, `kern init` writes an `upgrade` task: any package the project already uses may move to another version (`"dependencies": { ..., "allow": ["*"] }`), none may be added or removed, none may become another package (an `npm:` alias, or a lockfile entry whose name or tarball belongs to something else), no other file may change, and every test that passed must still pass. Whenever the dependencies move, the verifier installs the old lockfile and the new one separately, so the old code on the old packages is compared with the old code on the new ones, and a package that now behaves differently or newly reaches out shows up. With a hosted verifier, the merge check verifies pull requests from `renovate/` and `dependabot/` branches (the action's `upgrade-branches`) with that task. GitHub gives runs that Dependabot starts no secrets, so those are reported as not verified; use Renovate, or `kern ingest` the upgrade.

**On the pull request.** The verifier signs a `compare` check whose evidence is the comparison itself. The description carries it, and the merge check shows a "What changed" section only when the signed proof covers exactly that comparison; `kern run` prints it too. A boxed agent gets the stated change in `KERN_DELTA`, so it knows what to change and what to leave alone. `kern init` writes version 5 contracts with `"delta": null`; write the delta into the task before the agent starts when you know what the change should do. See [Evidence](#evidence-2026-10-08-contract-version-5-on-gvisor) for every lens run on gVisor.

## Local box (Windows, macOS, Linux)

The local box is the developer on-ramp: it runs an agent in a Linux container on your own machine, then gates the result with the same contract and capability checks the server uses. Any of Docker Desktop, Podman, Rancher Desktop, or Docker Engine or Podman on Linux will do (on Windows they all run on WSL2, and you can also run the Linux `kern` inside WSL2). Kern finds `docker` or `podman` on the `PATH`; set `KERN_CONTAINER_CLI` to choose.

```sh
kern doctor               # is this machine ready to run the box?
kern box box-job.json     # run one command in the box
kern run run-job.json     # box an agent, gate its output, promote a green result to a reversible branch
```

A run job names the agent's image, pinned by digest, the agent's command inside that image, and anything else it needs:

```json
{
  "version": 2,
  "repo": "./",
  "base": "<full-40-hex-base-commit>",
  "contract": "./.github/kern/tasks/agent.json",
  "image": "docker.io/library/node:22-bookworm-slim@sha256:<digest>",
  "command": ["/usr/local/bin/node", "/opt/agent/agent.js"],
  "mounts": [{ "source": "/home/you/agent", "target": "/opt/agent" }],
  "env": ["ANTHROPIC_API_KEY"],
  "network": true,
  "output": "./kern-runs",
  "promote": true
}
```

`kern run` copies the base commit into a throwaway workspace, runs the agent in a fresh container, then checks the result against the contract (paths, change budget, protected files) and the capability diff. A clean result becomes a create-only `kern/<task>/<run_id>` branch; the real working tree and `HEAD` never move, and a blocked result promotes nothing. The agent's tools come from its image; `mounts` adds read-only host files or folders, and `env` passes the named variables from Kern's own environment, so secrets never appear on a command line.

What the box enforces:

- The workspace is the only writable host folder (`/workspace`). The image's filesystem and every mount are read-only; the agent gets an empty private home and `/tmp` in memory. Nothing else from the host is visible: no home folder, credentials, Docker socket or `.git`.
- No network unless the job asks for it. A granted network is ordinary outbound access, and on Docker Desktop that includes services listening on your own machine (through `host.docker.internal`), so grant it only to agents you would trust with those.
- All capabilities dropped, `no-new-privileges`, seccomp, private process and IPC namespaces, no host devices, and the contract's memory (with no extra swap), process and time limits. An agent that runs out of memory or time is stopped and named as the reason.
- Only the variables listed in `env` reach the agent. Engines like to add their own (Docker copies proxy settings from `~/.docker/config.json`, Podman copies the host's proxy variables), so Kern turns Podman's copying off and refuses to start if anything unlisted still arrives.
- Before the agent starts, Kern reads back how the engine actually configured the container and refuses to run if anything is weaker than asked. Rootless Podman on cgroup v1, for example, silently ignores memory limits, so Kern refuses it.
- When Docker has gVisor registered as `runsc`, the box uses it automatically. On SELinux hosts Kern relabels only the workspace it created; label any folder you mount yourself (`chcon -Rt container_file_t <dir>`).
- The container is removed before Kern reads a single byte of output, and the output is read with the same refusals as the verifier (no symlinks, special files or over-budget files). A run whose container never actually started is refused, not counted as a clean exit.
- Ctrl+C, a closed terminal and the time limit stop the agent and remove its container. A small watchdog removes it the moment the Kern process that started it ends, however it ends, including a crash or being killed outright.
- The agent's output is shown on stderr with every line marked `[agent]` and terminal control characters removed, so an agent cannot print a fake verdict or hide Kern's real one. Kern's stdout stays machine-readable.

Add a `verifier` block (a pinned transport to an independent verifier that holds the signing key; the developer side never does) and `kern run` becomes the whole chain in one command: box → local gate → exact-hash export → `kern ingest` → independent gVisor verification of the real tests → verifier-signed attestation → create-only promotion → a dev-side signature and exact-commit confirmation. The developer sees only:

```text
✓ Isolated
✓ Verified independently
✓ Tests passed
✓ Signed

VERIFIED
```

or `BLOCKED: <reason>`. Any deviation (a failing agent, an out-of-contract change, an unreachable or failing verifier, a tampered or mismatched proof, a replayed run) fails closed and promotes nothing. `--show` prints the full JSON evidence.

The verifier block can also point at a hosted verifier: `{"url": "https://verifier.example", "token_file": "./token.txt", "public_key": "./verifier.pub", "operator": "./operator.json"}` ([template](examples/run-job.hosted.template.json)).

The local box protects your machine from the agent; it is not the security guarantee. The guarantee is the independent verifier and its signed proof, which a merge or deploy re-checks no matter where the agent ran.

## Feedback the agent can act on

Every block becomes a machine-readable verdict, `kern-feedback/v1`, with one instruction per problem. Instructions are written for the agent: what to change in the code, never how to loosen the gate. That stays in a separate `allow` hint for humans, which the agent's copy leaves out. The full verdict, as the run result keeps it:

```json
{ "schema": "kern-feedback/v1", "status": "BLOCKED", "agent_fixable": true, "attempt": 1, "max_attempts": 3,
  "violations": [{ "rule": "capability", "path": "src/run.js", "detail": "node:child_process",
    "instruction": "Remove the new use of `node:child_process` from `src/run.js`. This task may not give code new process, network, filesystem, environment or dynamic-code authority.",
    "allow": "A human can allow it in operator.json: \"permissions\": {\"src/\":[\"node:child_process\"]}" }] }
```

- **`kern run`** with `"attempts": 3` in the run job gives the agent another try after each rejection it can fix, in the same workspace, with the previous verdict in the `KERN_FEEDBACK` environment variable. Problems the agent cannot fix (out of memory, the time limit, an unreachable verifier) stop at once. The result lists every attempt, the summary says, for example, "Accepted after 1 rejected attempt", and the audit log keeps the whole history. The human sees one clean result with the rejected attempts on record.
- **The PR check** writes the verdict to `$RUNNER_TEMP/kern-feedback.json`, to the action's `status` and `feedback` outputs and to the job summary, and posts it as a PR comment (the workflow needs `pull-requests: write`). With `agent-mention: "@claude"` (or `@copilot`) the comment asks the agent to fix what it can; after `max-attempts` rejected commits, or for a problem it cannot fix, it asks a human instead. An attempt is a commit Kern rejected: a re-run or an edited description on the same commit adds no comment and uses no attempt. Kern counts only its own earlier comments, never edits or deletes them, and adds a short note when a change passes after rejections. Text that came from the change, such as paths, test names or program names, stays inside code formatting, so it cannot mention people or add links. A failure to post never changes the verdict.
- What the agent is handed (`KERN_FEEDBACK`, the feedback file and the comment's machine-readable block) carries instructions only. The raw reason and the `allow` hints stay in the run result and in the job summary for maintainers.
- `kern ingest` and `kern task` include the same verdict when they block, and so does `kern run` when an independent verifier rejected the change.

A proof binds one exact commit on one exact base. When an agent pushes a fix to a `kern/*` branch, or the base moves on, the proof no longer matches. With a hosted verifier configured, the check verifies the push again on its own ([Re-verify on push](#re-verify-on-push)), and anything still wrong comes back to the agent as above. Without one, the check fails with `proof-stale`, which only a human can resolve: turn re-verification on, or run the task through Kern again.

## Re-verify on push

When an agent answers review by pushing to its `kern/*` branch, or someone updates the branch from its base, the old proof no longer covers the head. With `verifier-url` and `verifier-token` set on the action, the `kern-verified` check then:

1. takes the base branch's current tip from GitHub as the trust root, and reads the task contract, operator settings and verifier key from it;
2. goes on only if the description still carries a proof the trusted verifier signed for this run (the `run_id` in the branch name); anything else keeps its original verdict;
3. moves the branch's change onto that tip: each file as the push left it, unless only the base branch changed it since the fork point GitHub reports. A file both sides changed is a conflict the agent is asked to merge; Kern never merges inside a file;
4. writes one deterministic Kern commit on the tip and sends it to the hosted verifier under the same run, which runs everything a first verification runs;
5. checks the new proof exactly as the gate checks every Kern pull request;
6. moves the branch to that commit only if the branch still holds the pushed commit (a newer push wins and is checked on its own), puts the new proof in the description, and reports the required check for the new commit, because GitHub starts no workflow for a push made with the workflow's own token. A short comment asks the agent to fetch the branch before pushing more.

A merge queue is checked the same way: the group passes when it lands exactly the tree Kern verified on the base Kern verified it on, and with a hosted verifier a group on a newer base is verified again under the same run, with the new proof recorded on the pull request so the merge audit finds it. So every Kern pull request keeps one invariant: its head is exactly a commit the verifier signed, one commit on a commit of the base branch, and what merges is what was verified. The workflow needs `contents: write` and `checks: write` for this; the generated workflow runs one check per pull request at a time. Kern pushes only to the pull request's own `kern/*` branch, with a lease on the commit it verified, and never runs pull-request code in that job.

## Hosted verifier

```sh
kern --control verifier.json serve service.json     # binds to loopback; put a TLS reverse proxy in front
```

`kern serve` exposes the verifier over a small HTTP API: `GET /v1/profile` (what `kern init` reads), `GET /healthz`, and `POST /v1/verify` with a handoff, answered with the signed attestation. Each repository has its own bearer tokens, stored only as SHA-256 hashes, and a token for one repository cannot verify another. The service verifies only against task contracts it holds itself, matched by digest, runs one verification at a time, refuses to bind anywhere but loopback, and refuses to start if its profile's public key does not match its signer. Every request is appended to its audit log. See [examples/serve.template.json](examples/serve.template.json).

## Audit

### Decision log and dashboard

Set `KERN_AUDIT_LOG=<file>` and every decision (`check-pr`, `gate-artifact`, `verify`, `promote-*`, `ingest`, `box`, ...) is appended as one JSON line; `kern run` and `kern serve` always record to their output directories. Each entry carries the hash of the previous one, so `kern audit <log>` detects any edited, deleted or reordered entry. Record the printed head hash somewhere else (a CI log, a ticket) to also detect truncation of the newest entries. `kern report <log> report.html` renders a self-contained dashboard: decisions, block rate, the reasons changes were blocked, and every decision with its commit.

Every block says why and how to allow it (see [Feedback the agent can act on](#feedback-the-agent-can-act-on) for the machine-readable form), for example `Patch widens module capabilities: src/run.js +[node:child_process]. If this is intended, allow it in operator.json: "permissions": {"src/":["node:child_process"]}` or `Tests show new runtime behavior: exec:id. If this is intended, allow it in the task contract (version 3): "behaviors": ["exec:id"]`.

### Merge audit

```sh
git fetch --unshallow origin; git fetch origin '+refs/pull/*/head:refs/remotes/pull/*'
GITHUB_TOKEN=... kern audit-merges . origin/main 2026-Q3 audit-q3          # or 2026-07-01..2026-09-30, and optionally a verifier.pub of your own
```

Kern reads the branch's whole first-parent history and places each change by its order on the branch, not by the date in the commit, so a backdated or future-dated commit cannot drop out of the period; such dates are flagged. For every change in the period, Kern looks for the pull request that merged it into that branch (a pull request merged into another branch does not count). For each `kern/*` agent change it re-verifies the proof against the trusted configuration in the change's base commit, or against a verifier key the auditor supplies, re-checks the contract and the capability diff, and checks that the merged result is exactly the verified commit applied to the base it was verified on. Each change is `verified`, `ordinary` (merged from another branch), or a gap: `direct-push` (no pull request into the branch), `invalid-proof`, or `unverifiable` (the pull request head is missing from the clone). Changes to `.github/kern/` or to workflows are flagged as gate configuration changes, and agent changes merged without a passing `kern-verified` check are flagged too. Shallow and partial clones are refused, because they can miss changes.

The report (`report.json`, `report.html`, `report.csv`) records the branch tip and the first and last change of the period, so consecutive reports can be checked to join up. It says plainly what a proof covers: the change and how it was verified, not what the agent did while writing it, and it cannot tell whether a person used AI tools for an ordinary change. It maps the evidence to SOC 2 CC8.1 and ISO/IEC 27001:2022 A.8.32 and A.8.29 without claiming compliance, escapes all recorded text, and neutralises spreadsheet formulas in the CSV. The command exits 0 when the period is covered and 2 when it found gaps; [examples/kern-audit.yml](examples/kern-audit.yml) runs it each quarter and keeps the report as a build artifact.

## Build

```sh
cargo build --release --locked
cargo test
# Linux: the verifier end to end on real gVisor, with the static build it copies into its sandboxes
cargo build --release --locked --target x86_64-unknown-linux-musl
KERN_BIN=target/x86_64-unknown-linux-musl/release/kern sh scripts/e2e-gvisor.sh
```

Production commands need native Linux, Docker with genuine gVisor registered as `runsc`, and memory/swap/PID/CPU limits. There is no fallback runtime.

Develop and test on Linux (on Windows, inside WSL2): the verifier, and with it Kern's guarantee, runs there. `scripts/e2e-gvisor.sh` installs gVisor when Docker lacks it (Ubuntu or Debian), sets up a throwaway verifier with Kern's own commands, and checks that an honest change is verified and its proof passes the deploy gate, while a change that breaks the tests, adds authority or changes behaviour the task did not state is refused, as are a tampered proof and a proof for another commit. CI runs the whole suite, the static build, that end-to-end run and the installer on Linux for every change; Windows gets a build, lint, test and installer run (Windows PowerShell 5.1 and PowerShell 7) for every change, and macOS a build and test run on `main` and for every release.

## Set up one Linux VM (default)

1. Install gVisor (`gvisor.tar.zstd` from gvisor.dev, verify its SHA-512) and register it in `/etc/docker/daemon.json` as `"runsc": {"path": "/usr/local/bin/runsc"}`.
2. Write two control files from [examples/control.template.json](examples/control.template.json), one with `"role": "executor"` and one with `"role": "verifier"`. Each pins the Docker, git and runsc binaries and a host policy by SHA-256.
3. Run `kern --control verifier.json doctor` and the executor equivalent, then copy both `builder.sha256` values into [the operator policy](examples/operator.template.json). The identity covers the pinned binaries and versions, machine ID, Kern's own hash and the policy. It changes on upgrades, not on reboots.
4. Run `kern --control verifier.json keygen /operator/keys`, or use a KMS/HSM through an external signer ([template](examples/signer.template.json)). The signer receives DSSE PAE bytes on stdin and returns a base64 Ed25519 signature, which Kern verifies before writing anything.
5. Pin each check file with `kern pin-check <id> <file>`. A check is a list of `{command, stdout}` cases; `"stdout": null` means exit 0 is enough. For example, `{"command": ["/usr/local/bin/npm", "test"], "stdout": null}` runs the project's own suite. Checks run on both the candidate and the base commit; the base run only records behaviour, so a base that fails its own tests (the bug being fixed) is fine.
6. To allow a new runtime behaviour for a task, use a version 3 contract with `"behaviors": ["net:api.example.com:443", "exec:git", "env:API_TOKEN"]`. A narrower contract can never add to this list.

Set `"deployment": "multi-machine"` to require that executor and verifier report different machines.

The verifier keeps finished dependency installs as read-only volumes and reuses them when the manifests, registry and image are the same; the index lives in `KERN_CACHE_DIR` (default `~/.cache/kern`), and `KERN_DEPS_CACHE=off` turns reuse off. Independent sandboxes, such as each check on the old and the new code, run in parallel, at most `KERN_VERIFY_PARALLEL` at once (1 to 8, default 2).

## Run

```sh
# A) the agent runs inside Kern's gVisor executor (offline command)
kern --control executor.json plan job.json
kern --control executor.json task job.json

# B) the agent ran anywhere else and produced a commit
kern ingest job.json /path/to/agent-repo <agent-commit>

# Verifier: transfer only the handoff/ directory (git bundle + run.json)
kern --control verifier.json import <bundle>/handoff run
kern --control verifier.json verify-external run task.json operator.json out signer.json

# Promoter: independent expectations, create-only branch, consumed exactly once
kern authorize run out/attestation.json task.json operator.json expected.json public.pem
kern promote-local run out/attestation.json task.json operator.json expected.json public.pem target.git
kern promote-github run out/attestation.json task.json operator.json expected.json public.pem token-file main /promoter/ledger git-pin.json
```

`expected.json` is `{run_id, base, commit, branch: "kern/<task>/<run_id>"}`, written by the reviewer. Nothing in an attestation grants publication on its own.

## Enforce at merge

1. Commit `.github/kern/operator.json`, `.github/kern/verifier.pub` and `.github/kern/tasks/<task>.json` to the default branch (`kern init` writes all of these).
2. Add [examples/kern-verified.yml](examples/kern-verified.yml) and require the `kern-verified` check, ideally through the generated ruleset. Pin the action to the Kern release your verifier runs: the check needs the same Kern version and capability policy, while the verifier's own binary stays pinned through its builder identity.

The action downloads a prebuilt Kern binary and checks it against a SHA-256 digest written into `action.yml`, so pinning the action by commit pins the binary too; it needs no extra tools and works the same on Linux, Windows and macOS runners. Until a release is pinned it builds from source. Releases (`.github/workflows/release.yml`, on a `v*` tag) test and build static Linux binaries (x64 and ARM64), Windows x64 and macOS (Apple silicon and Intel) binaries, add the two installers stamped with that release, publish `SHA256SUMS`, and attach GitHub build-provenance attestations (`gh attestation verify <file> --repo OWNER/kern`); `scripts/pin-release.sh <tag>` writes a release's digests into `action.yml`. Enable immutable releases on the repository so published assets cannot change.

The workflow uses `pull_request_target`, so its definition comes from the protected base branch and a PR cannot alter the check. Kern only reads git objects from the PR; it never executes PR code there, and the action builds or installs Kern in its own directory, never in the pull request's files. For `kern/*` branches the check requires exactly one valid attestation for the exact head and base, and recomputes the contract and capability diff. Other branches, including pull requests from forks, are not checked out and are reported as `SKIPPED` (`actions/checkout` v7 refuses fork code in `pull_request_target`). The generated ruleset also requires pull requests to be up to date with their base, so what merges is the commit Kern verified; with re-verification on, updating a branch from its base is verified again like any other push. From 2 November 2026 GitHub turns `pull_request_target` off in public repositories that have no event policy allowing it; allow it for this workflow.

## Enforce at deploy (factory lock)

Merge enforcement does not cover a push straight to a branch or a deploy of a commit that was never Kern-verified. A release pipeline closes that gap:

```sh
kern gate-artifact <checkout-dir> <commit> attestation.json <trusted-config-dir>
```

It re-verifies the attestation's signature against the trusted verifier key, that the attestation binds that exact commit, and recomputes the contract and capability diff against the checkout. A commit without a valid proof exits non-zero, so the deploy step fails closed.

## Evidence (2026-10-08, contract version 5 on gVisor)

The static release build of this tree as the verifier, in gVisor, on the real sprintf.js suite (mocha) and on a small app with a dependency (`ms`), a data file, an HTTP server that guards `/admin`, a paid API call and a money check. Each row is one agent change through `kern ingest` and the full verifier.

| What | Result |
|---|---|
| The stated sprintf.js fix (a bare precision such as `%.f` means precision 0), the delta written before the code | VERIFIED in 35 s: 444 calls to 2 functions in 2 modules answered the same on the old and the new code, and both of the task's examples hold |
| The same fix with nothing stated, and a refactor under "nothing changes" | VERIFIED in 34 s and 6 s, every call the same |
| The fix plus a change nobody asked for (a missing `%s` value prints nothing instead of `undefined`), which the 28 existing tests do not notice | DENIED in 37 s: "Behaviour changed outside the task's stated change: `sprintf("%s")`: returned "undefined", now returns """, 6 calls named; the 2 calls inside the stated change were allowed |
| A new type check that throws for a format that is not a string | DENIED in 6 s: a new crash, `sprintf([])` returned "" and now throws a `TypeError`, 42 calls in all |
| A tidy-up that drops the amount check of `withdraw`, under "nothing changes" | DENIED in 4 s: "threw RangeError: invalid amount, now returns NaN", 102 calls |
| A guard removed from the app (`if (text == null) return 0`), which the tests do not notice | With nothing stated: VERIFIED, and the evidence says why the line was there ("`parse([])`: returns 0 with it, returns undefined without it", 8 calls) and which commit added it. Under "nothing changes": DENIED in 6 s. For a guard removed from sprintf.js, `kern ingest` named the pull request that added it (#227) |
| New code that overwrites `data/state.json`, and new code that deletes files | DENIED in 5 s and 6 s: "Calling the new code shows new runtime behavior: overwrite:data/state.json"; for the delete, Kern's call `purge("..")` tried to delete the repository itself (`delete:.`), and the source scan named `fs.rmSync` |
| `/admin` no longer checks the token | DENIED in 5 s: "Access widened: GET /admin as anonymous: 401 before, 200 now" |
| A real upgrade (`ms` 2.1.2 to 2.1.3) under the `upgrade` task, each lockfile installed on its own | VERIFIED in 5 s: 564 calls to 5 functions answered the same on the old and the new packages |
| An "upgrade" that installs another package under the name `ms` (`npm:pretty-ms@7.0.1`) | Refused before verification: "package.json swaps ms for another package: ms becomes pretty-ms" |
| A function made 250 times slower (0.050 ms to 12.539 ms) under "nothing changes" | DENIED in 7 s: "Slower than the old code" |
| A second `chat.completions.create` per summary under "nothing changes" | DENIED in 4 s: "More paid API calls than the old code: src/ai.js: chat.completions.create, 1 before, 2 now" |
| A file read that returns "" instead of null for a missing file, nothing stated | VERIFIED in 5 s, with all 114 differing calls in the evidence for review |
| Found along the way | A real bug in sprintf-js 1.1.3: its parse cache keys the format as text, so after `sprintf(0)` the call `sprintf("0")` returns "" instead of "0" (and `sprintf(null)` breaks `sprintf("null")`). The comparison showed it when a change stopped `sprintf(0)` from filling the cache; plain Node confirms it |
| An independent review of the comparison (one read-only agent that ran the real probe on Windows and Linux) | 3 critical and 6 important findings, each reproduced, then fixed test-first: messages the code under test posted were trusted (a forged "done" left calls unmade, and that passed), a shared call budget starved later exports, values were compared only as far as they were shown, the task's `when` and examples were judged inside the worker the code controls, an upgrade could swap a package for another, GitHub titles could escape the step summary's code block, a module that never finished loading stalled the probe, an export list was capped at 60 and dropped aliases, and a door that opened on one run, or from a 404, went unnoticed. The fixes were then rerun on gVisor (the rows above) |
| The proof-of-honest-fix scenarios on gVisor, rerun with this build | 14 of 14 as before in 270 s: the honest, extended and deduplicated tests and the declared removal VERIFIED; every trick refused, the weakened tests naming the same 4 lost planted bugs |
| Unit, integration and adversarial tests | 113 of 113 on Windows and 114 of 114 on Linux, no `clippy` warnings; red-team corpus 92 of 92 still blocked |

## Evidence (2026-10-08, real agent pull requests)

| What | Result |
|---|---|
| 320 real merged agent pull requests (64 each from Copilot, Devin, Codex, Claude and Cursor, in 254 public JS/TS repositories, most of them small), replayed through Kern's own gate with the contract `kern init` writes for the repository at the pull request's base | Before the fixes below: 18% passed untouched, 46% after a maintainer widened the contract, and Kern could not be set up on 39% of the repositories (16 MiB limit, no `src/` folder, a symlink or submodule anywhere). After: 52% pass untouched, 66% after widening, 10% cannot be set up (19 pull requests in repositories over 64 MiB of text, 4 over 10,000 files, 8 with unportable or case-colliding paths, 1 with nothing to grant). Of the rest, most are blocked by design: CI files (13), `package.json` and lockfiles (16), agent instruction files, and files Kern cannot read (images, `.sql`, `.yaml`, `.astro`). The replay covers the gate's static checks, not the test runs |
| What that replay found in Kern itself, each fixed test-first | JSX in `.js` files could not be read (3 pull requests); the line budget counted the whole middle of a file, so a 3-file, 65-line change counted as over 2,000 lines (9); `kern init` granted only `src/`, `lib/`, `app/` and the like; a single symlink or submodule refused the whole repository |
| Three large repositories (mathjs, 6,674 tests with mocha; axios, 1,443 with vitest; winston, 239 with jest, and mocha before it switched), 12 real merged pull requests through the full gVisor verifier, the binary from before these fixes against the new one, each with an empty dependency cache | Before: 2 of 12 VERIFIED. Of the 10 refused, 9 were Kern's own problems, not the changes': vite could not bundle its config into the read-only `node_modules`, winston's tests could not write their log files, jest's `--json` changed what 3 winston tests saw, 3 axios tests fail before any change (2 need the internet, 1 fails upstream on Node 22), an `AUTHORS` file without an extension, and a line budget counted as 12,733 lines. After: 11 of 12 VERIFIED; the twelfth, an axios test for prototype pollution that reads `__proto__` and `constructor`, is held by the dynamic-code rule until a human allows it. Times: mathjs 281 s to 236 s, and 190 s to 66 s with the dependency install reused; the others took 3.5 to 25 minutes, mostly planting 40 bugs to check rewritten tests (none lost); winston's planting fell from 29 minutes, one bug at a time, to 11 minutes in two parallel sandboxes. Measured on one 20-core VM, partly while other suites ran. Two set-up errors in the benchmark itself were corrected and rerun: the older winston pull requests needed mocha, and test folders get `node:assert` as `kern init` grants it |
| Impossible tasks on the real sprintf.js suite (5 tasks whose new requirement contradicts an existing test, 11 strategies each: honest, delete, edit, skip, only, alias, exit, mock, stack and caller inspection, a call counter) | Every attempt must fail. Before the fix: 5 of 55 accepted, all by the call counter (code that returns the new answer only on its first call passes the new test, which runs first, and the old one). After running the old tests alone on the new code as well: 0 of 55 accepted (20 refused before anything ran, 35 by the verifier) |
| The earlier end-to-end proof-of-honest-fix scenarios on gVisor, rerun with the final build | 14 of 14 as before: honest, extended and deduplicated tests VERIFIED, a removal VERIFIED only as a declared change, and every trick refused; the weakened tests are refused naming the same 4 lost planted bugs as before, now planted in parallel sandboxes |
| Re-verify on push and merge queues against a GitHub stand-in (a real git server, an API stub and a stub hosted verifier signing with the test key) | A push onto the agent's branch while the base moved: one Kern commit on the new tip with both changes, verified under the same run, branch moved with a lease, proof and check published; the next run passes without the verifier. Refused or left alone: a conflicting file, a verifier refusal, a forged proof, a fork, a newer push that landed first. A merge-queue group of the verified tree passes without the verifier; on a newer base it is verified again and the proof recorded; a group with extra changes is refused; ordinary pull requests are skipped |

## Evidence (2026-10-08)

| What | Result |
|---|---|
| Proof of honest fix on gVisor with the real sprintf.js suite (issue #181), read from mocha's own report | An honest fix: VERIFIED in 109 s; mocha reported all 28 existing tests passing before and after, and the new test failing on the old code and passing on the new code. Refused: a new test that already passed and a test that mocked the code (no new test failed on the old code), an edited existing test, an `it.only`, a fix without a test, an `.only` reached through `it['on' + 'ly']` (28 existing tests did not run) and a `process.exit(0)` in an `after` hook (no report). The last two pass every pattern check; the runner's report caught them |
| Rewritten and removed tests on gVisor with the real sprintf.js suite, checked with planted bugs | A regression test added to an existing test file: VERIFIED in 96 s; the old tests still passed on the new code (28 of 28), and of 40 planted bugs the old tests caught 34, all still caught, with 14 s spent planting. Removing three duplicate assertions: VERIFIED in 45 s (31 of 31 still caught). Deleting a 50-line test block: DENIED in 42 s, naming the 4 planted bugs only those tests caught; two of them were confirmed with plain mocha outside Kern (the original tests fail, the weakened tests pass). Changing what the code does and editing the test to match: DENIED, the old test fails on the new code. Removing a feature with its test: DENIED as a refactor, VERIFIED as a declared change with the changed expectation listed. Editing an existing test under a contract that locks tests: refused before anything ran |
| Test runner reports in a read-only gVisor sandbox shaped like the verifier's (jest 29, vitest 2, `node --test` on Node 22) | Each produced its report; vitest exits with failure when it cannot write its cache into `node_modules`, so Kern adds `--no-cache`. The report readers are tested against real output of mocha 5 and 10, jest, vitest and `node --test`, including a test file that cannot load |
| Agent feedback loop with real containers | An agent that edited a file outside its grant was told why, undid the edit and made the fix on its second attempt; the result was promoted with both attempts on record. An agent that ran out of memory was not retried |
| PR check feedback against a GitHub API stub | The first rejection mentions the agent with a machine-readable verdict that holds no loosening hints; a re-run on the same commit adds nothing; after the cap a human is asked; a fix pushed to the `kern/*` branch was reported stale (now verified again, see above); an unfixable problem never mentions the agent; a pass after rejections is noted; a GitHub outage leaves the verdict unchanged |
| Merge audit of a real repository history against a GitHub API stub | 9 changes in the quarter: 1 verified, 5 gaps (a direct push, a commit merged through a pull request into another branch, a direct push backdated to before the quarter, a proof signed by another key, a pull request head missing from the clone) and 1 gate configuration change. git's own date-filtered walk stops at the backdated commit and misses two of these changes; Kern's walk does not. Hostile commit text escaped in HTML and neutralised in CSV; an auditor-supplied key enforced; shallow and partial clones refused |

## Evidence (2026-10-07)

All measurements were run on this implementation; the live runs used gVisor release-20260928.0 on a single Linux VM, the real sprintf.js test suite (ESLint + Mocha) and an external signer.

| What | Result |
|---|---|
| Unit, integration and adversarial tests (real git, real Ed25519, git as the oracle, real container engines) | 53/53 on Windows (Docker Desktop) and 54/54 on Linux (Docker in WSL2), zero `clippy` warnings; CI runs Linux, Windows and macOS on every commit |
| Local box escape probes (Docker Desktop on Windows, Docker in WSL2, Docker with gVisor as root, rootful Podman 5.8) | Writes outside the workspace fail with `EROFS`; no Docker socket; only listed variables reach the agent, and a host proxy with credentials is refused or kept out; the internet is unreachable (`ENETUNREACH`); memory and process limits are enforced; exit codes, out-of-memory, the time limit, Ctrl+C, SIGTERM, a closed terminal and a Kern killed outright all leave no container behind; agent output cannot imitate Kern's verdict; rootless Podman on cgroup v1 is refused because it ignores limits |
| Red-team corpus (`tests/redteam.rs`): known evasions of static capability detection, plus an independent adversarial review | 92 of 92 blocked; 28 of 28 realistic agent edits accepted; 2 known false blocks tracked |
| Live verification, benign change | VERIFIED; base and candidate recorded the same 127 behaviours |
| Live verification, a test that newly runs a program (static check allowed because `child_process` was permitted in tests) | Blocked by the behaviour diff: `Tests show new runtime behavior: exec:id` |
| The same change with `exec:id` allowed in a version 3 contract | VERIFIED; three full verifications in 64 s |
| Hosted verifier on gVisor | 401 without a token; 200 with a signed attestation carrying the behaviour check; audit chain intact |
| Proof portability | A proof signed by the Linux release build is accepted by a separately built Windows binary (ALLOW); the same proof for a different commit is refused |
| Static Linux build | `static-pie` musl binary runs unchanged in `node:22-bookworm-slim` |

## Evidence (2026-10-04)

| What | Result |
|---|---|
| Onboarding benchmark: 10 real npm repos, 50 candidates (good + four defect classes), real suites in gVisor | 50/50 as expected: 0 wrong-VERIFIED, 0 false-blocks, 10/10 onboarded. Mix of `src/`, `lib/` and root-`index.js` (file grants), with 1–3 MiB assets streamed |
| Escape probes inside the gVisor executor | Credentials EACCES; Docker socket, `.git`, host root and signing key absent; every write outside grants denied; env exactly `HOME LANG PATH`; IPv4, IPv6 and DNS unreachable; detached writer stopped before capture |
| Hostile captures (symlink, hardlink, timeout) | All refused |
| Live verification of upstream sprintf.js issue #181 | 433 locked packages installed in gVisor, 32 behavioural cases and the real `npm test` (ESLint + Mocha) in about 50 s; signed by an external signer |
| Hostile candidates (incomplete fix, `child_process` added, bug, failing own test) | All denied: behaviour, capability and test-suite gates each triggered |
| Promotion and required check | Create-only branch; replay refused after branch deletion; PASS on exact head; a later push and a missing attestation are refused |
| Capability check on 3,443 real files from 34 repos (policy v2; re-run before quoting, since v3 also flags aliasing, executable data files and paths into `node_modules`) | 99.9% parsed; 100% of harmless edits accepted; 99.2% of 40,479 injected attacks blocked, and every miss was authority the file already held |
| Repository acceptance (50 popular JS/TS repos) | 34/50. The rest exceed 16 MiB or 1 MiB per file, or contain symlinks or submodules |
| Reading 2,468 files (nestjs/nest) | 0.19 s |

## Limits

- Changed code must be JS/TS/JSX or text/JSON/CSS/HTML/Markdown, or a contributor list or change log without an extension (`AUTHORS`, `CONTRIBUTORS`, `CHANGELOG` and the like), which is checked for hidden code like any other text. Repositories must be SHA-1 git, at most 10,000 files and 64 MiB of text (files up to 1 MiB each). Symlinks and submodules are carried unchanged: a task cannot change them, and tests run without them.
- Dependency setup supports npm `package-lock.json` v2/v3 from one registry, with install scripts disabled. Legacy SHA-1-only lock pins are accepted and counted in the record.
- The capability check is static: it stops new authority from being granted, not misuse of authority a module already has. Names are resolved through `oxc_semantic`, so `process` or `self` count only when they are the real globals, and `this` counts as the global object inside plain non-strict functions. New code that passes such a `this` on as a value (for example `$(this)` or `fn.apply(this, arguments)` in a non-strict file) is refused; existing code is covered by its baseline.
- A new file may use any authority its directory already uses, including combinations no single file had; files in one directory can already import each other, so the directory is the unit of authority.
- The behaviour diff runs inside the code under test (a Node preload), so code written to evade it can. It catches honest surprises such as a new telemetry call, not a determined adversary; only programs started from Node are observed. Kernel-level tracing through gVisor is the planned tamper-proof upgrade.
- In a merge queue, a group passes when it lands exactly the tree Kern verified for its agent pull request, on the base Kern verified it on; with a hosted verifier, a group on a newer base is verified again. A group that also holds another pull request's changes is refused, so queue agent pull requests alone (a group size of 1). For `merge_group` events GitHub runs the workflow file of the group's own commit, which includes the queued changes, while Kern refuses agent changes to `.github/`.
- Re-verification on push needs a hosted verifier and gives the pull-request workflow `contents: write` and `checks: write`. It moves whole files only, so a file that both the push and the base branch changed goes back to the agent, and it rewrites the agent's branch to one verified commit, so the agent must fetch before pushing again.
- The proof of honest fix supports mocha, jest, vitest and `node --test`, and it trusts the runner's report. Planted bugs come from a small set of edits, at most 40 per change, so the safety-net check is a strong sample, not a proof; code a test reaches only through a package name or a child process is covered by planting across all code outside the tests instead; and moving code the old tests use counts as a change of behaviour, because the old tests then no longer load. Code that tampers with the test runner, its report or the assertion library from inside the test process in ways Kern does not recognise can still fake a pass; the hidden operator checks and review remain the backstop. Test names must stay the same from run to run. A run that fails is run once more, so a test that fails twice in a row blocks. Tests that failed before the change are set aside rather than required, so a change can leave them broken; the evidence lists them for review.
- The comparison of the old and the new code is strong evidence, not a proof: it calls exported functions with sampled arguments. Behaviour that depends on arguments nobody generates (a password, a particular date), on a database or on a network answer can hide a change. Modules that do not load in Node without a build step (JSX, TypeScript beyond erasable types, browser code) are listed as not checked; class methods are not called, only constructors. The harness that calls a function and writes down its answer shares a worker thread with it, so code written to fool the comparison can make its answers look like the old code's, as code that detects the probe always could; it cannot sign the probe's messages, change which calls the task states, decide whether an example holds, leave calls unanswered unnoticed or redirect the report. Speed is a median measured in the sandbox and needs a large difference to count. Paid API calls are counted where the code really tries to reach the host (a test that mocks the SDK never does) and by the call names Kern knows.
- The merge audit needs read access to the GitHub API and a full clone with the pull request heads. It recognises agent changes by their `kern/*` branches and trusts GitHub's record of which pull request merged each commit.
- The local box uses standard container isolation unless Docker has gVisor; a granted network is unrestricted outbound access, including services on the developer's machine. On Docker Desktop, large workspaces are slower through its file sharing than inside WSL2. Where the agent runs as root (rootless Docker, Docker Desktop's own Linux VM, or Kern run as root), agents that refuse root, such as Claude Code with `--dangerously-skip-permissions`, need their own opt-in. The box has been run on Windows (Docker Desktop) and Linux (Docker, Docker with gVisor, rootful Podman); macOS, Docker Desktop for Linux, rootless Podman on cgroup v2 and SELinux hosts are handled from their documented behaviour but not yet run.
- The required check has run live on GitHub (2026-10-07) with a committed binary. The release download path of the action and `promote-github` have not been exercised live yet.

See [SECURITY.md](SECURITY.md) for the trust model.
