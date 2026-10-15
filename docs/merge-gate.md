# Kern merge gate

The merge gate decides whether an AI agent's change may be merged, and proves it with a signature that a GitHub check verifies.

In short:

1. A **task contract** says what the agent may change.
2. A **verifier** checks the change in a sealed gVisor sandbox: its scope, any new powers, the project's real tests, and the old code's answers against the new code's.
3. The verifier **signs** the result. The `kern-verified` check on GitHub verifies the signature before merge.

Back to the [overview](../README.md).

## What the gate checks

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

Kern has no release yet. Build it from source:

```sh
git clone https://github.com/jastijayakrishna/Kerner.git
cd Kerner
cargo build --release --locked      # the binary is target/release/kern
```

Once a release is published, one command installs it:

```sh
# Linux or macOS
curl -fsSL https://github.com/jastijayakrishna/Kerner/releases/latest/download/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://github.com/jastijayakrishna/Kerner/releases/latest/download/install.ps1 | iex
```

The installer picks the build for your machine (Linux x64 or ARM64, macOS Apple silicon or Intel, Windows x64, which Windows 11 on Arm runs as well), checks it against the release's `SHA256SUMS`, tries it, and only then puts it in place, so a failed install never leaves a broken `kern` behind. It installs to `~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\kern`, added to your PATH). Set `KERN_VERSION=v0.3.0` to install a particular release and `KERN_INSTALL_DIR` to choose the folder. Every release file also carries a GitHub build-provenance attestation: `gh attestation verify <file> --repo jastijayakrishna/Kerner`.

Nothing else is needed for setup, the merge gate or audits. Running an agent in Kern's own local box needs a container engine (below), and a verifier needs Linux with gVisor.

## Quick start (about 10 minutes)

```sh
# On the verifier: export its public settings once (operator template + public key)
kern profile operator.json /operator/keys/public.pem > profile.json

# In a repository: generate the task contract, operator config, verifier key, workflow and ruleset
kern init . profile.json jastijayakrishna/Kerner@COMMIT_SHA            # or: kern init . https://verifier.example
git add .github && git commit -m "Add the Kern merge gate"
gh api -X POST repos/OWNER/REPO/rulesets --input .github/kern/ruleset.json   # OWNER/REPO: your repository
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

`kern init` writes this section with `"kind": "feature"` when it can tell which runner the repository uses; set `"kind": "bugfix"` for a bug-fix task. New tests must live in files the suite command runs. See [Evidence](evidence.md#evidence-2026-10-08) for a run on a real project in gVisor.

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

**On the pull request.** The verifier signs a `compare` check whose evidence is the comparison itself. The description carries it, and the merge check shows a "What changed" section only when the signed proof covers exactly that comparison; `kern run` prints it too. A boxed agent gets the stated change in `KERN_DELTA`, so it knows what to change and what to leave alone. `kern init` writes version 5 contracts with `"delta": null`; write the delta into the task before the agent starts when you know what the change should do. See [Evidence](evidence.md#evidence-2026-10-08-contract-version-5-on-gvisor) for every lens run on gVisor.

## Run the agent in any sandbox

Kern's guarantee does not depend on where the agent ran: the verifier checks the result again from scratch in gVisor, and the merge and deploy gates check the verifier's signature. So run the agent wherever you already isolate it, and hand its commit to Kern:

- **Docker Sandboxes** (`sbx`, free, a microVM per agent with its own network policy, on Windows 11, Apple silicon Macs and Linux with KVM), the agent's own sandbox (Claude Code's `/sandbox`, Codex's sandbox), a dev container or a cloud agent: commit the agent's work, then `kern ingest job.json <repo> <commit>` (see [Run](#run)).
- **Kern's local box** (below), when you want Kern to start the agent, gate its output and promote it in one command.

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

The verifier block can also point at a hosted verifier: `{"url": "https://verifier.example", "token_file": "./token.txt", "public_key": "./verifier.pub", "operator": "./operator.json"}` ([template](../examples/run-job.hosted.template.json)).

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

`kern serve` exposes the verifier over a small HTTP API: `GET /v1/profile` (what `kern init` reads), `GET /healthz`, and `POST /v1/verify` with a handoff, answered with the signed attestation. Each repository has its own bearer tokens, stored only as SHA-256 hashes, and a token for one repository cannot verify another. The service verifies only against task contracts it holds itself, matched by digest, runs one verification at a time, refuses to bind anywhere but loopback, and refuses to start if its profile's public key does not match its signer. Every request is appended to its audit log. See [examples/serve.template.json](../examples/serve.template.json).

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

The report (`report.json`, `report.html`, `report.csv`) records the branch tip and the first and last change of the period, so consecutive reports can be checked to join up. It says plainly what a proof covers: the change and how it was verified, not what the agent did while writing it, and it cannot tell whether a person used AI tools for an ordinary change. It maps the evidence to SOC 2 CC8.1 and ISO/IEC 27001:2022 A.8.32 and A.8.29 without claiming compliance, escapes all recorded text, and neutralises spreadsheet formulas in the CSV. The command exits 0 when the period is covered and 2 when it found gaps; [examples/kern-audit.yml](../examples/kern-audit.yml) runs it each quarter and keeps the report as a build artifact.

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
2. Write two control files from [examples/control.template.json](../examples/control.template.json), one with `"role": "executor"` and one with `"role": "verifier"`. Each pins the Docker, git and runsc binaries and a host policy by SHA-256.
3. Run `kern --control verifier.json doctor` and the executor equivalent, then copy both `builder.sha256` values into [the operator policy](../examples/operator.template.json). The identity covers the pinned binaries and versions, machine ID, Kern's own hash and the policy. It changes on upgrades, not on reboots.
4. Run `kern --control verifier.json keygen /operator/keys`, or use a KMS/HSM through an external signer ([template](../examples/signer.template.json)). The signer receives DSSE PAE bytes on stdin and returns a base64 Ed25519 signature, which Kern verifies before writing anything.
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
2. Add [examples/kern-verified.yml](../examples/kern-verified.yml) and require the `kern-verified` check, ideally through the generated ruleset. Pin the action to the Kern release your verifier runs: the check needs the same Kern version and capability policy, while the verifier's own binary stays pinned through its builder identity.

The action downloads a prebuilt Kern binary and checks it against a SHA-256 digest written into `action.yml`, so pinning the action by commit pins the binary too; it needs no extra tools and works the same on Linux, Windows and macOS runners. Until a release is pinned it builds from source. Releases (`.github/workflows/release.yml`, on a `v*` tag) test and build static Linux binaries (x64 and ARM64), Windows x64 and macOS (Apple silicon and Intel) binaries, add the two installers stamped with that release, publish `SHA256SUMS`, and attach GitHub build-provenance attestations (`gh attestation verify <file> --repo jastijayakrishna/Kerner`); `scripts/pin-release.sh <tag>` writes a release's digests into `action.yml`. Enable immutable releases on the repository so published assets cannot change.

The workflow uses `pull_request_target`, so its definition comes from the protected base branch and a PR cannot alter the check. Kern only reads git objects from the PR; it never executes PR code there, and the action builds or installs Kern in its own directory, never in the pull request's files. For `kern/*` branches the check requires exactly one valid attestation for the exact head and base, and recomputes the contract and capability diff. Other branches, including pull requests from forks, are not checked out and are reported as `SKIPPED` (`actions/checkout` v7 refuses fork code in `pull_request_target`). The generated ruleset also requires pull requests to be up to date with their base, so what merges is the commit Kern verified; with re-verification on, updating a branch from its base is verified again like any other push. From 2 November 2026 GitHub turns `pull_request_target` off in public repositories that have no event policy allowing it; allow it for this workflow.

## Enforce at deploy (factory lock)

Merge enforcement does not cover a push straight to a branch or a deploy of a commit that was never Kern-verified. A release pipeline closes that gap:

```sh
kern gate-artifact <checkout-dir> <commit> attestation.json <trusted-config-dir>
```

It re-verifies the attestation's signature against the trusted verifier key, that the attestation binds that exact commit, and recomputes the contract and capability diff against the checkout. A commit without a valid proof exits non-zero, so the deploy step fails closed.

## Results and limits

- Every measured result: [evidence.md](evidence.md)
- What the gate does not do yet: [limits.md](limits.md)
