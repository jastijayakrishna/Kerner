# Kern security review: scope

Kern decides whether an AI agent's change may be merged, and proves the decision with an Ed25519-signed in-toto attestation that a GitHub required check and a deploy gate re-verify. This brief is for an outside reviewer. The trust model is in [SECURITY.md](../SECURITY.md); setup and known limits are in the [README](../README.md).

## What a finding means

In order of impact:

1. **A forged or misapplied proof:** `check-pr`, `gate-artifact` or `authorize` accepts an attestation that the trusted verifier did not sign for that exact commit, base, contract and check set (`src/attest.rs`, `src/promote.rs`).
2. **A sandbox escape or key exposure:** code under test or an agent leaves the gVisor executor or verifier sandbox, reads the signing key, or influences what is captured (`src/sandbox.rs`, `src/flow.rs`).
3. **New authority accepted:** a patch adds authority (process, network, filesystem, environment, dynamic code, a new package, a path outside the repository) and is still VERIFIED (`src/capability.rs`, `src/git.rs` `changes`). `tests/redteam.rs` lists the evasions already covered.
4. **A contract bypass:** a write outside the granted paths, to a protected file, or over budget is accepted (`src/git.rs`, `src/local.rs` `read_workspace`).
5. **Hosted verifier compromise:** an auth bypass, one tenant verifying another's repository, request parsing flaws, or resource exhaustion (`src/serve.rs`).
6. **Windows box escape:** reading or writing outside the workspace and granted tools, reaching the network when it was not granted, or reusing another run's grants (`src/winbox.rs`, unsafe Win32 code).
7. **Undetected audit-history edits,** beyond the documented truncation limit, or script injection into `kern report` output (`src/audit.rs`).
8. **Promotion replay,** or publishing anything other than the authorized commit to a create-only branch (`src/promote.rs`).

## Untrusted inputs

- Agent repositories and commits (`kern ingest`).
- Handoff bundles and `run.json` (`kern import`, `POST /v1/verify`).
- Pull-request events and bodies (`check-pr`).
- Attestation files (`gate-artifact`, `authorize`).
- Workspace contents written by a boxed agent (`kern run`).
- HTTP requests to `kern serve`.
- All code executed inside sandboxes.

## Trusted inputs

- Control, operator, contract, check and signer files.
- The pinned Docker, git and runsc binaries.
- The base branch of the protected repository.

## Out of scope

gVisor, Docker, the host kernel and GitHub itself, except where Kern misconfigures or misuses them.

## How to build and test

- `cargo build --release --locked`, then `cargo test` on Windows and Linux. CI does both on every commit.
- The live gVisor path needs Linux, Docker with `runsc`, and the control files described in the README.
- `tests/redteam.rs` is the place to add capability evasions.

## Suggested methods

- **Fuzzing:** the HTTP/1.1 request parser in `serve.rs`, bundle import (`git::import_handoff`), attestation parsing, and `read_commit`'s object stream parser.
- **Differential testing:** run the capability analyzer against Node's real module resolution and execution, especially aliasing, data files executed by `require`, and declaration files.
- **Unsafe code review:** ACL and AppContainer handling in `winbox.rs`.
- **Behaviour diff:** confirm it fails closed when records are missing. It is documented as advisory against hostile code.
