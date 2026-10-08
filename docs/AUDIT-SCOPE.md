# Kern security review: scope

Kern decides whether an AI agent's change may be merged, and proves the decision with an Ed25519-signed in-toto attestation that a GitHub required check and a deploy gate re-verify. This brief is for an outside reviewer. The trust model is in [SECURITY.md](../SECURITY.md); setup and known limits are in the [README](../README.md).

## What a finding means

In order of impact:

1. **A forged or misapplied proof:** `check-pr`, `gate-artifact` or `authorize` accepts an attestation that the trusted verifier did not sign for that exact commit, base, contract and check set (`src/attest.rs`, `src/promote.rs`).
2. **Re-verification abuse:** a push re-verified under any trust root other than the base branch's tip, a branch moved to a commit no verifier signed or over a newer push, a pull request re-verified without a proof for its run, a merge that drops or reverts a base-branch change, a merge-queue group accepted with content no proof covers, the verifier token sent anywhere but the hosted verifier, or the GitHub token leaking from git commands (`src/reverify.rs`, `git::remote`).
3. **A sandbox escape or key exposure:** code under test or an agent leaves the gVisor executor or verifier sandbox, reads the signing key, or influences what is captured (`src/sandbox.rs`, `src/flow.rs`).
4. **New authority accepted:** a patch adds authority (process, network, filesystem, environment, dynamic code, a new package, a path outside the repository) and is still VERIFIED (`src/capability.rs`, `src/git.rs` `changes`). `tests/redteam.rs` lists the evasions already covered.
5. **A contract bypass:** a write outside the granted paths, to a protected file, or over budget is accepted (`src/git.rs`, `src/local.rs` `read_workspace`).
6. **Hosted verifier compromise:** an auth bypass, one tenant verifying another's repository, request parsing flaws, or resource exhaustion (`src/serve.rs`).
7. **Local box escape:** an agent that reads or writes outside its workspace and read-only mounts, reaches the network when it was not granted, sees an environment variable the job did not list, outlives its time limit, Ctrl+C or the Kern process that started it (the `__reap` watchdog), runs in a container whose limits or isolation the engine weakened without Kern refusing, or prints output that passes for Kern's own verdict (`src/container.rs`, including its small unsafe signal handling).
8. **Undetected audit-history edits,** beyond the documented truncation limit, or script injection into `kern report` output (`src/audit.rs`).
9. **Promotion replay,** or publishing anything other than the authorized commit to a create-only branch (`src/promote.rs`).
10. **A dishonest fix accepted:** a candidate that skips or stops existing tests, gets a test that passed before set aside as already failing, rewrites or removes tests so that a behaviour change hides or a planted bug the old tests caught goes uncaught, adds a test that passes without the fix, makes a run look complete without the runner's real report, or otherwise passes the `honest-fix` check without a test the runner reports failing before the change and passing after it (`src/proof.rs`, and the honest-fix block in `src/flow.rs`).
11. **Feedback abuse:** a block message that tells an agent how to loosen the gate, agent-controlled text that is explained as a different block or reaches an instruction outside code formatting, a PR comment that mentions people or renders links from agent-controlled text, a retry count that someone else can change, or a feedback failure that changes a verdict (`src/feedback.rs`).
12. **A misleading merge audit:** a change reported as verified whose merged content differs from the verified commit, a gap that goes unreported (including through a false commit date or a pull request into another branch), or report text that runs in a browser or a spreadsheet (`src/merges.rs`, `promote::verify_merged`).

## Untrusted inputs

- Agent repositories and commits (`kern ingest`).
- Handoff bundles and `run.json` (`kern import`, `POST /v1/verify`).
- Pull-request events and bodies (`check-pr`).
- Attestation files (`gate-artifact`, `authorize`).
- Workspace contents written by a boxed agent (`kern run`).
- HTTP requests to `kern serve`.
- GitHub API responses (pull request bodies and comments, commit associations, check runs) read by the PR check and the merge audit.
- Test files and test runner results in the honest-fix proof.
- All code executed inside sandboxes.

## Trusted inputs

- Control, operator, contract, check and signer files.
- The pinned Docker, git and runsc binaries.
- On a developer machine, the container engine and the agent image the developer pins.
- The base branch of the protected repository.

## Out of scope

gVisor, Docker, the host kernel and GitHub itself, except where Kern misconfigures or misuses them.

## How to build and test

- `cargo build --release --locked`, then `cargo test` on Windows, macOS and Linux. CI does all three on every commit; set `KERN_REQUIRE_CONTAINER=1` where a container engine is available so the local box tests cannot be skipped.
- The live gVisor path needs Linux, Docker with `runsc`, and the control files described in the README.
- `tests/redteam.rs` is the place to add capability evasions.

## Suggested methods

- **Fuzzing:** the HTTP/1.1 request parser in `serve.rs`, bundle import (`git::import_handoff`), attestation parsing, and `read_commit`'s object stream parser.
- **Differential testing:** run the capability analyzer against Node's real module resolution and execution, especially aliasing, data files executed by `require`, and declaration files.
- **Planted bugs:** whether the planted set misses behaviour a removed test protected, and whether a test can tell a planted run from a real one.
- **Re-verification:** race a push against a running re-verification, retarget or rename branches mid-run, and feed `check-pr` events whose head, base and body disagree with GitHub's state.
- **Test runner reports:** feed Kern's report readers hostile and malformed mocha, jest, vitest and TAP output, and compare what they read with what the runners actually ran across their versions.
- **Unsafe code review:** the signal and console-control handlers in `container.rs`, and `quiesce` in `sandbox.rs`.
- **Behaviour diff:** confirm it fails closed when records are missing. It is documented as advisory against hostile code.
