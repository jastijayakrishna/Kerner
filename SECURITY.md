# Security model

Kern binds a merge decision to evidence about one exact commit. It does not make agents, patches or hosts safe in general.

## Trust

- **Untrusted:** agent code and output, repository text read by the agent, executor handoff data, and PR bodies.
- **Trusted per role:** the host, kernel, Docker daemon, gVisor, the pinned Docker/git/runsc binaries, the node image, operator policy, and the role's storage.
- **Verifier-only:** check definitions and signing authority. The executor holds only the verifier's public key.
- **Promoter-only:** publication credentials, the consumed-run ledger, and the reviewer's `expected.json`.

## Guarantees

- **Executor sandbox.** Read-only root, project and Kern binary. Writes go only to tmpfs for the granted directories. No network, no host credentials, no Docker socket, no git metadata. UID 1000, all capabilities dropped, `no-new-privileges`, and memory/swap/PID/CPU/time limits. The daemon's own inspection of each container is checked before it starts.
- **Capture.** After the agent exits, the in-sandbox runner (PID 1) kills every other process in the namespace before reading output. It refuses symlinks, hardlinks, special files and over-budget output, and opens every file with `O_NOFOLLOW`. The container must be removed before any byte is accepted.
- **Handoff.** A git bundle with exactly two refs (base and candidate) and no prerequisites, at most 64 MiB. Objects are fsck-checked on import, and the candidate's manifest must match before anything is labelled imported.
- **Verification.** The verifier re-checks paths, protected files, change budget and the capability diff. Its own cases and the project's test suite run on the exact commit, each in a fresh no-network gVisor sandbox. The expected answers never enter the sandbox.
- **Attestation.** An Ed25519 DSSE envelope over an in-toto Statement (SLSA verification summary). It binds commit, base, manifest, contract, Kern version, verifier configuration, builder identities (which pin each role's exact Kern binary), deployment, image, dependency tree digest and the complete check set. A checker needs the same Kern version and capability policy as the verifier, not the same binary. Verifiers rebuild the canonical statement and require byte equality. Lifetime is one hour for promotion.
- **Promotion.** Values come from independent expectations only. Branches are create-only and run-specific. An authorization is reserved durably before the first mutation and stays consumed even after a failure or branch deletion.
- **Behaviour diff.** The verifier runs every check on the base commit as well, records network destinations, DNS names, started programs and environment reads in both runs, and refuses a candidate that newly reaches the network, starts a new program, enumerates the environment or reads a secret-looking variable, unless the task contract lists it. The result is a signed `behavior` check.
- **Hosted verifier.** `kern serve` binds to loopback only (TLS belongs in a reverse proxy), stores bearer tokens as SHA-256 hashes per repository, authenticates before revealing which repositories it serves, verifies only against contracts it holds (matched by digest), runs one verification at a time, caps connections and request sizes, and refuses to start if its published key differs from its signer.
- **Audit log.** Decisions are appended as hash-chained JSON lines under an OS file lock; `kern audit` detects edited, removed or reordered entries, and `kern report` escapes all recorded text.
- **Windows box.** A fresh AppContainer identity per run; grants are revoked afterwards and cannot be reused by a later run even if Kern is interrupted. Workspaces get edit rights but not permission or ownership changes. Denied paths are never modified: a job is refused if one lies inside a grant or is readable by every app container.
- **Merge check.** Runs from the base branch's workflow. It re-verifies the signature against the trusted key in the base commit and requires attested head == PR head and attested base == PR base. It recomputes the contract and capability checks.

## Known limits

- **Agents can abort their own run.** Under gVisor, an agent can signal the runner (PID 1) and abort its run. The run fails closed and nothing is captured.
- **Dependency setup has network access.** It sees only `package.json` and the lockfile, runs `npm ci --ignore-scripts` with every package pinned to one registry host by integrity, and forces TLS. Packages with only legacy SHA-1 pins are counted and disclosed, not rejected.
- **Single-VM mode shares one kernel.** Executor, verifier and promoter share a kernel and a Docker daemon, so a gVisor escape there could reach the signing key. Use `multi-machine` deployment and a KMS/HSM signer where that matters. Machine separation is checked from self-reported identities; it is not hardware attestation.
- **The capability check is syntactic.** It blocks new authority. Misuse of authority a module already holds needs tests and review. Identifiers are resolved with `oxc_semantic`; `tests/redteam.rs` tracks known evasions and the accepted false blocks. A new file is judged against its whole directory's authority.
- **The behaviour diff is advisory against hostile code.** Its recorder runs inside the code under test, which can tamper with it, and it only observes programs started from Node. It reliably catches unintended new behaviour, not deliberate evasion; gVisor-level tracing is the tamper-proof follow-up.
- **The audit chain cannot detect truncation of its newest entries on its own.** Anchor the head hash printed by `kern audit` elsewhere.
- **The Windows box sets no memory or process limits yet.**
- **Passing checks is not proof of correctness or intent.** Code can be tailored to known tests.
- **Publication is not atomic.** GitHub push and PR creation are separate requests. Mismatches fail and leave state for manual reconciliation. CI triggered by a pushed branch is outside Kern's control.

## Reporting

Report vulnerabilities privately to the repository owner. Never include credentials.
