# Kern

**Admission control for AI-written code.** Any coding agent (OpenShell, Claude Code, Codex, Copilot, Devin) proposes a change. Kern decides whether that exact commit may be merged, and proves it with a signed in-toto attestation that a GitHub required check re-verifies.

Sandboxes such as OpenShell control what an agent can *do while it runs*. Kern controls what the agent's *output may become*:

- **Task contract.** The agent may change only the granted directories, within a change budget counted with a real line diff. Lockfiles, `.github/`, `.env*` and other dependency or CI files are always protected.
- **Capability diff.** JS/TS/JSX are parsed with `oxc`. A patch is refused if it gives a module new authority, such as `child_process`, `fetch`, `process.env`, `eval`, dynamic `require`, or a new package. Unchanged authority never blocks an edit. A text, Markdown, CSS, HTML or `.d.ts` file that parses as JavaScript is checked the same way, since `require()` executes it, and loading a declaration file at runtime counts as new authority.
- **Independent verification.** A separate verifier runs the project's real test suite and operator-owned cases on the exact commit, each in a fresh gVisor sandbox with no network. Dependencies come only from the committed lockfile.
- **Behaviour diff.** The verifier runs the same checks on the base commit too and compares what the code actually did: network destinations, DNS names, programs started and secret-looking environment reads. A change that newly reaches out is blocked unless the task contract allows it, which catches a module that already had network or process access starting to use it somewhere new.
- **Proof of honest fix.** When the task contract asks for it, existing tests are locked and the verifier reads the test runner's own report on the exact commit: every test that ran before must still run and pass, none may be newly skipped, and for a bug fix a new test must fail on the old code and pass on the new code. Editing, deleting or skipping tests, stopping them from running and changing the test runner's settings are refused, and a test that mocks the fixed code away does not count.
- **Feedback for the agent.** Every block comes with a machine-readable verdict and plain instructions an agent can act on, never hints on how to loosen the gate. `kern run` hands it back to the agent and retries on its own, the PR check posts it as a comment, both with a retry cap, and every rejected attempt stays on record.
- **Merge audit.** `kern audit-merges` re-verifies the proof of every change merged in a period and lists the changes that skipped the gate, as JSON, HTML and CSV for auditors.
- **Signed proof at merge time.** The verifier emits an in-toto Statement with a SLSA verification-summary predicate. The `kern-verified` check re-verifies it against the PR head, so any later push invalidates it.

What Kern guarantees: an agent's change cannot reach `main` unless it stays inside its contract, adds no authority the code did not already have, passes the real tests on the exact commit, and shows no new runtime reach, all proven by a signature only the verifier can make. What it does not guarantee: that the code is correct or well-intended. Code can be tailored to known tests, and authority a module already holds can still be misused. Kern narrows what an agent can change; review still decides whether the change is good.

```text
agent (any sandbox) ──commit──▶ kern ingest ──bundle──▶ kern verify (gVisor) ──in-toto──▶ kern promote ──▶ draft PR
                                                                                                     └▶ kern-verified required check
```

Kern is one static Rust binary. The same binary is the CLI, the verifier and the in-sandbox runner.

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
"tests": { "paths": ["test/"], "kind": "bugfix", "runner": "mocha", "command": ["/workspace/node_modules/.bin/mocha"] }
```

- **Kern reads the runner's own report.** `runner` is `mocha`, `jest`, `vitest` or `node`, and Kern adds the matching reporter to `command` (`--reporter json`, `--json`, `--reporter=json --no-cache` since vitest cannot write its cache in the read-only sandbox, or `--test-reporter=tap` right after `--test`), so it knows test by test what ran, passed, failed or was skipped. The command may not choose a reporter of its own, and vitest must run as `vitest run`. Exactly one report must come out of each run: output a test prints is ignored, and a second report is refused.
- **Before anything runs**, existing files under `tests.paths` are locked (changing or deleting one is refused, so new tests go in new files), no change may touch the test runner's settings (`.mocharc*`, `jest.config*`, `vitest.config*`, `vite.config*`, `setupTests*`, Babel configuration and similar), new test files may not skip or focus tests (`.only`, `.skip`, `.todo`, `xit`/`fit` and the like, or `skip`/`only`/`todo` options), and code outside the tests may not use test-runner globals such as `describe` or `expect`, change an assertion library such as `node:assert`, or check whether tests are running (`JEST_WORKER_ID`, `VITEST` and similar). These checks run at every gate and give the agent early feedback.
- **Then the verifier runs the whole suite, in gVisor without network, at most three times**, each within the contract's time limit: the existing suite on the old code, everything on the new code, and for `"kind": "bugfix"` everything on the old code with the new test files added. On the new code nothing may fail, every test that ran before must still run and pass, no test may be newly skipped, and the command itself must succeed. A bug fix also needs a new test that the runner reports *failing* on the old code: a test that already passes there does not show the bug, which also rules out a test that mocks the fixed code away, and a test file that cannot even load on the old code does not count. `"feature"` skips the third run, and `"refactor"` needs no new test.
- Because the report shows the effect, it does not matter how a trick is written: an `.only` reached through an alias, a skip hidden in the code under test, a test renamed through a data file or an early `process.exit` all show up as tests that did not run, or as a run with no report.
- The result is a signed `honest-fix` check. Its evidence, `honest-fix.json`, records each run, how many existing tests still pass, and every new test with its result on the old and the new code.

`kern init` writes this section with `"kind": "feature"` when it can tell which runner the repository uses; set `"kind": "bugfix"` for a bug-fix task. New tests must live in files the suite command runs. See [Evidence](#evidence-2026-10-08) for a run on a real project in gVisor.

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

A proof binds one exact commit on one exact base. When an agent pushes a fix to a `kern/*` branch, or the base moves on, the check fails with `proof-stale`, which the agent can fix: the comment asks it to run the task through Kern again, which verifies the new change and opens a new pull request. So the PR loop needs an agent that can run Kern; `kern run` with `attempts` closes the loop on its own.

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
```

Production commands need native Linux, Docker with genuine gVisor registered as `runsc`, and memory/swap/PID/CPU limits. There is no fallback runtime.

## Set up one Linux VM (default)

1. Install gVisor (`gvisor.tar.zstd` from gvisor.dev, verify its SHA-512) and register it in `/etc/docker/daemon.json` as `"runsc": {"path": "/usr/local/bin/runsc"}`.
2. Write two control files from [examples/control.template.json](examples/control.template.json), one with `"role": "executor"` and one with `"role": "verifier"`. Each pins the Docker, git and runsc binaries and a host policy by SHA-256.
3. Run `kern --control verifier.json doctor` and the executor equivalent, then copy both `builder.sha256` values into [the operator policy](examples/operator.template.json). The identity covers the pinned binaries and versions, machine ID, Kern's own hash and the policy. It changes on upgrades, not on reboots.
4. Run `kern --control verifier.json keygen /operator/keys`, or use a KMS/HSM through an external signer ([template](examples/signer.template.json)). The signer receives DSSE PAE bytes on stdin and returns a base64 Ed25519 signature, which Kern verifies before writing anything.
5. Pin each check file with `kern pin-check <id> <file>`. A check is a list of `{command, stdout}` cases; `"stdout": null` means exit 0 is enough. For example, `{"command": ["/usr/local/bin/npm", "test"], "stdout": null}` runs the project's own suite. Checks run on both the candidate and the base commit; the base run only records behaviour, so a base that fails its own tests (the bug being fixed) is fine.
6. To allow a new runtime behaviour for a task, use a version 3 contract with `"behaviors": ["net:api.example.com:443", "exec:git", "env:API_TOKEN"]`. A narrower contract can never add to this list.

Set `"deployment": "multi-machine"` to require that executor and verifier report different machines.

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

The action downloads a prebuilt Kern binary and checks it against a SHA-256 digest written into `action.yml`, so pinning the action by commit pins the binary too; it needs no extra tools and works the same on Linux, Windows and macOS runners. Until a release is pinned it builds from source. Releases (`.github/workflows/release.yml`, on a `v*` tag) build static Linux, Windows and macOS binaries, publish `SHA256SUMS`, and attach GitHub build-provenance attestations (`gh attestation verify <file> --repo OWNER/kern`); `scripts/pin-release.sh <tag>` writes a release's digests into `action.yml`. Enable immutable releases on the repository so published assets cannot change.

The workflow uses `pull_request_target`, so its definition comes from the protected base branch and a PR cannot alter the check. Kern only reads git objects from the PR; it never executes PR code there, and the action builds or installs Kern in its own directory, never in the pull request's files. For `kern/*` branches the check requires exactly one valid attestation for the exact head and base, and recomputes the contract and capability diff. Other branches, including pull requests from forks, are not checked out and are reported as `SKIPPED` (`actions/checkout` v7 refuses fork code in `pull_request_target`). The generated ruleset also requires pull requests to be up to date with their base, so what merges is the commit Kern verified. From 2 November 2026 GitHub turns `pull_request_target` off in public repositories that have no event policy allowing it; allow it for this workflow.

## Enforce at deploy (factory lock)

Merge enforcement does not cover a push straight to a branch or a deploy of a commit that was never Kern-verified. A release pipeline closes that gap:

```sh
kern gate-artifact <checkout-dir> <commit> attestation.json <trusted-config-dir>
```

It re-verifies the attestation's signature against the trusted verifier key, that the attestation binds that exact commit, and recomputes the contract and capability diff against the checkout. A commit without a valid proof exits non-zero, so the deploy step fails closed.

## Evidence (2026-10-08)

| What | Result |
|---|---|
| Proof of honest fix on gVisor with the real sprintf.js suite (issue #181), read from mocha's own report | An honest fix: VERIFIED in 107 s; mocha reported all 28 existing tests passing before and after, and the new test failing on the old code and passing on the new code. Refused: a new test that already passed and a test that mocked the code (no new test failed on the old code), an edited existing test, an `it.only`, a fix without a test, an `.only` reached through `it['on' + 'ly']` (28 existing tests did not run) and a `process.exit(0)` in an `after` hook (no report). The last two pass every pattern check; the runner's report caught them |
| Test runner reports in a read-only gVisor sandbox shaped like the verifier's (jest 29, vitest 2, `node --test` on Node 22) | Each produced its report; vitest exits with failure when it cannot write its cache into `node_modules`, so Kern adds `--no-cache`. The report readers are tested against real output of mocha 5 and 10, jest, vitest and `node --test`, including a test file that cannot load |
| Agent feedback loop with real containers | An agent that edited a file outside its grant was told why, undid the edit and made the fix on its second attempt; the result was promoted with both attempts on record. An agent that ran out of memory was not retried |
| PR check feedback against a GitHub API stub | The first rejection mentions the agent with a machine-readable verdict that holds no loosening hints; a re-run on the same commit adds nothing; after the cap a human is asked; a fix pushed to the `kern/*` branch asks the agent to run Kern again; an unfixable problem never mentions the agent; a pass after rejections is noted; a GitHub outage leaves the verdict unchanged |
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
- Merge queues (`merge_group` events) are not supported yet.
- The proof of honest fix supports mocha, jest, vitest and `node --test`, and it trusts the runner's report. Code that tampers with the test runner, its report or the assertion library from inside the test process in ways Kern does not recognise can still fake a pass; the hidden operator checks and review remain the backstop. Test names must stay the same from run to run, a flaky test makes the proof fail, and each run can write only to `/tmp`.
- The merge audit needs read access to the GitHub API and a full clone with the pull request heads. It recognises agent changes by their `kern/*` branches and trusts GitHub's record of which pull request merged each commit.
- The local box uses standard container isolation unless Docker has gVisor; a granted network is unrestricted outbound access, including services on the developer's machine. On Docker Desktop, large workspaces are slower through its file sharing than inside WSL2. Where the agent runs as root (rootless Docker, Docker Desktop's own Linux VM, or Kern run as root), agents that refuse root, such as Claude Code with `--dangerously-skip-permissions`, need their own opt-in. The box has been run on Windows (Docker Desktop) and Linux (Docker, Docker with gVisor, rootful Podman); macOS, Docker Desktop for Linux, rootless Podman on cgroup v2 and SELinux hosts are handled from their documented behaviour but not yet run.
- The required check has run live on GitHub (2026-10-07) with a committed binary. The release download path of the action and `promote-github` have not been exercised live yet.

See [SECURITY.md](SECURITY.md) for the trust model.
