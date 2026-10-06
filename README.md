# Kern

**Admission control for AI-written code.** Any coding agent (OpenShell, Claude Code, Codex, Copilot, Devin) proposes a change. Kern decides whether that exact commit may be merged, and proves it with a signed in-toto attestation that a GitHub required check re-verifies.

Sandboxes such as OpenShell control what an agent can *do while it runs*. Kern controls what the agent's *output may become*:

- **Task contract.** The agent may change only the granted directories, within a change budget. Lockfiles, `.github/`, `.env*` and other dependency or CI files are always protected.
- **Capability diff.** JS/TS/JSX are parsed with `oxc`. A patch is refused if it gives a module new authority, such as `child_process`, `fetch`, `process.env`, `eval`, dynamic `require`, or a new package. Unchanged authority never blocks an edit.
- **Independent verification.** A separate verifier runs the project's real test suite and operator-owned cases on the exact commit, each in a fresh gVisor sandbox with no network. Dependencies come only from the committed lockfile.
- **Signed proof at merge time.** The verifier emits an in-toto Statement with a SLSA verification-summary predicate. The `kern-verified` check re-verifies it against the PR head, so any later push invalidates it.

```text
agent (any sandbox) ──commit──▶ kern ingest ──bundle──▶ kern verify (gVisor) ──in-toto──▶ kern promote ──▶ draft PR
                                                                                                     └▶ kern-verified required check
```

Kern is one static Rust binary. The same binary is the CLI, the verifier and the in-sandbox runner.

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
5. Pin each check file with `kern pin-check <id> <file>`. A check is a list of `{command, stdout}` cases; `"stdout": null` means exit 0 is enough. For example, `{"command": ["/usr/local/bin/npm", "test"], "stdout": null}` runs the project's own suite.

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

1. Commit `.github/kern/operator.json`, `.github/kern/verifier.pub` and `.github/kern/tasks/<task>.json` to the default branch.
2. Add [examples/kern-verified.yml](examples/kern-verified.yml) and require the `kern-verified` status in branch protection.

The workflow uses `pull_request_target`, so its definition comes from the protected base branch and a PR cannot alter the check. Kern only reads git objects from the PR; it never executes PR code there. For `kern/*` branches the check requires exactly one valid attestation for the exact head and base, and recomputes the contract and capability diff. Other branches are reported as `SKIPPED`.

## Enforce at deploy (factory lock)

Merge enforcement does not cover a push straight to a branch or a deploy of a commit that was never Kern-verified. A release pipeline closes that gap:

```sh
kern gate-artifact <checkout-dir> <commit> attestation.json <trusted-config-dir>
```

It re-verifies the attestation's signature against the trusted verifier key, that the attestation binds that exact commit, and recomputes the contract and capability diff against the checkout. A commit without a valid proof exits non-zero, so the deploy step fails closed.

## Evidence (2026-10-04)

All measurements were run on this implementation. The live runs used gVisor release-20260928.0 on a single Linux VM.

| What | Result |
|---|---|
| Unit and integration tests (real git, real Ed25519, git as the oracle) | 23/23, zero `clippy` warnings |
| Onboarding benchmark: 10 real npm repos, 50 candidates (good + four defect classes), real suites in gVisor | 50/50 as expected: 0 wrong-VERIFIED, 0 false-blocks, 10/10 onboarded. Mix of `src/`, `lib/` and root-`index.js` (file grants), with 1–3 MiB assets streamed |
| Escape probes inside the gVisor executor | Credentials EACCES; Docker socket, `.git`, host root and signing key absent; every write outside grants denied; env exactly `HOME LANG PATH`; IPv4, IPv6 and DNS unreachable; detached writer stopped before capture |
| Hostile captures (symlink, hardlink, timeout) | All refused |
| Live verification of upstream sprintf.js issue #181 | 433 locked packages installed in gVisor, 32 behavioural cases and the real `npm test` (ESLint + Mocha) in about 50 s; signed by an external signer |
| Hostile candidates (incomplete fix, `child_process` added, bug, failing own test) | All denied: behaviour, capability and test-suite gates each triggered |
| Promotion and required check | Create-only branch; replay refused after branch deletion; PASS on exact head; a later push and a missing attestation are refused |
| Capability check on 3,443 real files from 34 repos | 99.9% parsed; 100% of harmless edits accepted; 99.2% of 40,479 injected attacks blocked, and every miss was authority the file already held |
| Repository acceptance (50 popular JS/TS repos) | 34/50. The rest exceed 16 MiB or 1 MiB per file, or contain symlinks or submodules |
| Reading 2,468 files (nestjs/nest) | 0.19 s |

## Limits

- Changed code must be JS/TS/JSX or text/JSON/CSS/HTML/Markdown. Repositories must be SHA-1 git, at most 10,000 files and 16 MiB, with no symlinks or submodules.
- Dependency setup supports npm `package-lock.json` v2/v3 from one registry, with install scripts disabled. Legacy SHA-1-only lock pins are accepted and counted in the record.
- The capability check is syntactic: it stops new authority from being granted, not misuse of authority a module already has. Tests and review still matter.
- The GitHub promoter and Action are exercised locally against GitHub-shaped events; no live GitHub publication has been run yet.

See [SECURITY.md](SECURITY.md) for the trust model.
