# Kern

**Admission control for AI-written code.** Any coding agent (OpenShell, Claude Code, Codex, Copilot, Devin) proposes a change. Kern decides whether that exact commit may be merged, and proves it with a signed in-toto attestation that a GitHub required check re-verifies.

Sandboxes such as OpenShell control what an agent can *do while it runs*. Kern controls what the agent's *output may become*:

- **Task contract.** The agent may change only the granted directories, within a change budget. Lockfiles, `.github/`, `.env*` and other dependency or CI files are always protected.
- **Capability diff.** JS/TS/JSX are parsed with `oxc`. A patch is refused if it gives a module new authority, such as `child_process`, `fetch`, `process.env`, `eval`, dynamic `require`, or a new package. Unchanged authority never blocks an edit. A text, Markdown, CSS, HTML or `.d.ts` file that parses as JavaScript is checked the same way, since `require()` executes it, and loading a declaration file at runtime counts as new authority.
- **Independent verification.** A separate verifier runs the project's real test suite and operator-owned cases on the exact commit, each in a fresh gVisor sandbox with no network. Dependencies come only from the committed lockfile.
- **Behaviour diff.** The verifier runs the same checks on the base commit too and compares what the code actually did: network destinations, DNS names, programs started and secret-looking environment reads. A change that newly reaches out is blocked unless the task contract allows it, which catches a module that already had network or process access starting to use it somewhere new.
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

`kern init` reads the committed tree to find source and test directories, root-level code files and the lockfile, and writes a contract that grants those and nothing else. Build and test configuration (`*.config.js`), CI files, lockfiles and manifests stay out of reach. Every file it writes is validated by Kern's own parsers, and it refuses to overwrite an existing setup. The ruleset requires the `kern-verified` check from GitHub Actions only, so no other app can satisfy it, and it adds to existing branch protection rather than replacing it. Organizations can apply the same requirement to every repository with an org-level ruleset that requires the workflow.

## Developer box (Windows, no Docker, no admin)

The box is the developer-local on-ramp: it runs an agent inside a native Windows AppContainer, then gates the result with the same contract and capability checks the server uses — with no Docker, no VM and no administrator rights.

```sh
# Run one command inside the box (read-write on one workspace, tools granted, no network)
kern box box-job.json

# Box an agent, gate its output, and promote a green result to a reversible branch
kern run run-job.json
```

`kern run` materializes the base commit into a throwaway workspace, runs the agent inside the box, then checks the result against the contract (paths, change budget, protected files) and the capability diff. A clean result becomes a create-only `kern/<task>/<run_id>` branch; the real working tree and `HEAD` never move, and a blocked result promotes nothing. The box grants read-write to one workspace only, read-execute to the tool directories it is given, and no network unless the job asks for it. Isolation is enforced by OS ACLs and AppContainer network restrictions.

Add a `verifier` block (a pinned transport to an independent verifier that holds the signing key — the developer side never does) and `kern run` becomes the whole chain in one command: box → local gate → exact-hash export → `kern ingest` → independent gVisor verification of the real tests → verifier-signed attestation → create-only promotion → a dev-side signature and exact-commit confirmation. The developer sees only:

```text
✓ Isolated
✓ Verified independently
✓ Tests passed
✓ Signed

VERIFIED
```

or `BLOCKED: <reason>`. Any deviation — a failing agent, an out-of-contract change, an unreachable or failing verifier, a tampered or mismatched proof, a replayed run — fails closed and promotes nothing. `--show` prints the full JSON evidence.

Proven on Windows 11 (`tests/kern.rs`, and reproducibly by hand): inside the box, writing or deleting outside the workspace, reading a secret file, connecting to a local service, and reaching the internet are all denied, while the same actions succeed in an identical run outside the box. Granting the network restores internet access without restoring local-service or filesystem access.

Each run gets a fresh AppContainer identity, so grants left behind by an interrupted run (Ctrl+C, a crash) never carry over to later boxes; profiles of runs whose process is gone are cleaned up on the next launch. The agent gets edit rights in its workspace but cannot change permissions there. A box job's `denied` list names paths the box must never reach. Windows ignores deny entries for app containers, so Kern does not pretend to carve exceptions: it refuses a job where a denied path sits inside a granted directory or is readable by every app container, and it never modifies the denied path itself.

The verifier block can also point at a hosted verifier: `{"url": "https://verifier.example", "token_file": "./token.txt", "public_key": "./verifier.pub", "operator": "./operator.json"}` ([template](examples/run-job.hosted.template.json)).

## Hosted verifier

```sh
kern --control verifier.json serve service.json     # binds to loopback; put a TLS reverse proxy in front
```

`kern serve` exposes the verifier over a small HTTP API: `GET /v1/profile` (what `kern init` reads), `GET /healthz`, and `POST /v1/verify` with a handoff, answered with the signed attestation. Each repository has its own bearer tokens, stored only as SHA-256 hashes, and a token for one repository cannot verify another. The service verifies only against task contracts it holds itself, matched by digest, runs one verification at a time, refuses to bind anywhere but loopback, and refuses to start if its profile's public key does not match its signer. Every request is appended to its audit log. See [examples/serve.template.json](examples/serve.template.json).

## Audit log and dashboard

Set `KERN_AUDIT_LOG=<file>` and every decision (`check-pr`, `gate-artifact`, `verify`, `promote-*`, `ingest`, `box`, ...) is appended as one JSON line; `kern run` and `kern serve` always record to their output directories. Each entry carries the hash of the previous one, so `kern audit <log>` detects any edited, deleted or reordered entry. Record the printed head hash somewhere else (a CI log, a ticket) to also detect truncation of the newest entries. `kern report <log> report.html` renders a self-contained dashboard: decisions, block rate, the reasons changes were blocked, and every decision with its commit.

Every block says why and how to allow it, for example `Patch widens module capabilities: src/run.js +[node:child_process]. If this is intended, allow it in operator.json: "permissions": {"src/":["node:child_process"]}` or `Tests show new runtime behavior: exec:id. If this is intended, allow it in the task contract (version 3): "behaviors": ["exec:id"]`.

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

The workflow uses `pull_request_target`, so its definition comes from the protected base branch and a PR cannot alter the check. Kern only reads git objects from the PR; it never executes PR code there. For `kern/*` branches the check requires exactly one valid attestation for the exact head and base, and recomputes the contract and capability diff. Other branches are reported as `SKIPPED`.

## Enforce at deploy (factory lock)

Merge enforcement does not cover a push straight to a branch or a deploy of a commit that was never Kern-verified. A release pipeline closes that gap:

```sh
kern gate-artifact <checkout-dir> <commit> attestation.json <trusted-config-dir>
```

It re-verifies the attestation's signature against the trusted verifier key, that the attestation binds that exact commit, and recomputes the contract and capability diff against the checkout. A commit without a valid proof exits non-zero, so the deploy step fails closed.

## Evidence (2026-10-07)

All measurements were run on this implementation; the live runs used gVisor release-20260928.0 on a single Linux VM, the real sprintf.js test suite (ESLint + Mocha) and an external signer.

| What | Result |
|---|---|
| Unit, integration and adversarial tests (real git, real Ed25519, git as the oracle) | 47/47 on Windows, 41/41 on Linux, zero `clippy` warnings; CI runs both on every commit |
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

- Changed code must be JS/TS/JSX or text/JSON/CSS/HTML/Markdown. Repositories must be SHA-1 git, at most 10,000 files and 16 MiB, with no symlinks or submodules.
- Dependency setup supports npm `package-lock.json` v2/v3 from one registry, with install scripts disabled. Legacy SHA-1-only lock pins are accepted and counted in the record.
- The capability check is static: it stops new authority from being granted, not misuse of authority a module already has. Names are resolved through `oxc_semantic`, so `process` or `self` count only when they are the real globals, and `this` counts as the global object inside plain non-strict functions. New code that passes such a `this` on as a value (for example `$(this)` or `fn.apply(this, arguments)` in a non-strict file) is refused; existing code is covered by its baseline.
- A new file may use any authority its directory already uses, including combinations no single file had; files in one directory can already import each other, so the directory is the unit of authority.
- The behaviour diff runs inside the code under test (a Node preload), so code written to evade it can. It catches honest surprises such as a new telemetry call, not a determined adversary; only programs started from Node are observed. Kernel-level tracing through gVisor is the planned tamper-proof upgrade.
- Merge queues (`merge_group` events) are not supported yet.
- Windows box: no memory or process limits yet; agents get no writable home directory (Node's `os.homedir()` points at the real, unreadable profile), so agents that store state at home need one provided; PowerShell inside the box starts at `C:\`, so use absolute paths.
- The required check has run live on GitHub (2026-10-07) with a committed binary. The release download path of the action and `promote-github` have not been exercised live yet.

See [SECURITY.md](SECURITY.md) for the trust model.
