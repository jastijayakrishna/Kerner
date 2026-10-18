# Counterexample validation: measured repairs, unvalidated hunter

Date: 2026-10-18. Maturity: Validate. Branch: `codex/counterexample-validation`. Starting commit: `479030dd39e464e0c8eb9cbb58b10c9ce3b802f5`.

**The trust repairs have reproducible evidence. The AI architecture has not been validated. No live baseline-versus-hunter comparison, model invocation, production release, commit or push has occurred. The current six-PR configuration fails its feasibility preflight and must not be presented as evidence of AI detection value.**

## What was implemented

The experiment has a frozen protocol/ADR, repository-separated cohort and pilot, independently sourced build-contract packs, a typed proposal boundary, a sandbox worker, a bounded inference adapter and an all-PR scorer. The worker shares Kern's installer, sandbox and `suite::blockable`; experimental findings never update required checks or approve PRs. The deterministic core is shared and charged to both arms' budgets.

The following defects were repaired in the existing decision path:

| Defect | Resulting behavior |
| --- | --- |
| Empty or unknown evidence could rebuild as clear | Signing rejects missing mandatory evidence, unknown test statuses, missing result lists and missing comparability fields. Direct receipt construction treats unknown test status as unverified. |
| Passing tests erased unequal setup | Setup uncertainty survives verdict construction, folding and replay, including dependency resolution without a usable lockfile. Receipts suppress the green Tests line. |
| Workflow names inherited metadata exemptions | Executable workflows named `release.yml` or `funding.yml` remain reviewable. Existing non-executable funding/release metadata and image exemptions remain scoped separately. |
| Deleted public entry files disappeared from API analysis | Removed bindings are reported. A new surface that cannot be parsed is reported as unchecked rather than silently skipped. |
| Manifest-only export removal was missed | Changed package manifests participate in API analysis; removed export-map paths/conditions are visible. These are advisory findings, not proof that an intentional API change is wrong. |
| Approval omitted signed-text integrity validation | Approval invokes the existing markdown-digest check before success. The regression models a valid signature callback with altered signed text. |
| Type errors blocked after only one comparison | An enforced type-check failure without fresh confirmation becomes unverified. It remains visible; it does not become clear. Production type confirmation is not implemented in this milestone. |

The eight initial regressions failed on the starting implementation. During development, additional tests exposed oversized-output and off-domain-input gaps in the experimental verifier; both were fixed before any inference or PR execution.

The probe boundary now rejects oversized or unreadable results, malformed exit codes, crash/OOM evidence, wrong contract identities, modified oracles/commands/revisions/budgets, and inputs outside machine-enforced domains. Input-domain types are null-only, an approved nonempty string prefix, or explicitly approved JSON. The hunter cannot define its own domain or expected result.

## Measured historical repair audit

The original and repaired binaries rejudged the same archived evidence. The initial batch did not finish; it was interrupted after measurement showed sustained expensive CPU work.

| Measure | Observed |
| --- | --- |
| Selected archived checks | 4,606 |
| Completed paired checks before indexing | 3,202 |
| Unique commit pairs in those completed comparisons | 600 |
| Clear to unverified | 85, all with `test-setup` uncertainty |
| Blocked to look | 1, with the type failure moved to `types-new` uncertainty; existing review findings remain |
| New blocked decisions in those pairs | 0 |
| New clear decisions in those pairs | 0 |
| Missing paired outputs | 1,404 |

The demoted block was `farm-art15/receipt-tuliren__turbo-template__29`. This demonstrates removal of an unconfirmed block; it does not independently establish the underlying PR's correctness.

These are repeated stored checks, not 3,202 independent PRs. Replay rejudges recorded runtime evidence and retains recorded static-rule results. It does not rerun project code, recompute static findings, establish normative bug labels, or measure AI lift. The missing outputs remain in the denominator; their individual causes were not recovered before interrupting the batch.

Raw comparison: `target/counterexample-validation/replay-comparison-complete-denominator.json`.

## A reproduced performance bottleneck

Flakiness detection previously rebuilt a file's test-name sets once for every assertion. The Webpack archive under investigation contains 105,582 assertions across 196 result files. The implementation now indexes names by file once per report and compares those sets.

| Check | Result |
| --- | --- |
| New 50,000-assertion regression before optimization | Failed its 30-second deadline, exit 101 |
| Same regression after indexing | Passed in 2.79 seconds, exit 0 |
| Shared historical checks before/after indexing | 3,169 paired; zero decision changes |
| Indexed complete-archive attempt | 4,019 outputs within a 300-second deadline; then timed out at 300.235 seconds |

The indexing change removes one quadratic path. It does not resolve every replay bottleneck: the real Webpack case still had no completed output in the bounded full-corpus run. A complete replay gate has therefore **not passed**.

At 16:07:54 local time, the original and pre-index repaired workers had used 2,500.8125 and 2,492.359375 CPU seconds, with working sets of 370,413,568 and 358,834,176 bytes. Only those two task-owned processes were stopped. No Docker container or user service was stopped.

Next performance work should stream per-case progress/errors, apply per-case deadlines, and profile the remaining large-report path. The existing batch reporter defers errors until every worker finishes, which obscured individual incomplete-case reasons. Do not infer that the measured indexing fix identifies every hot function.

Raw evidence: `replay-performance.json`, `replay-indexed-timing.json`, `indexed-equivalence.json`, and the replay logs under `target/counterexample-validation/`.

## The proposed AI comparison has not run

The source pool is frozen at 328 PRs from 99 repositories: 69 development PRs from 19 repositories and 259 holdout PRs from 80 repositories. The source-pool digest is `c675348049ce3c40d3309a17cb8ed0cdddd13cc479dae2cd80ad01c1c2e5f2fb`.

The pilot selects two development and four holdout repositories by a second fixed hash and one PR per repository by a key hash. Selection did not depend on defect explanations, source outcomes or run success. Future fixes/reverts and labels are stored separately from hunter inputs. No inference has consumed any cohort data. Repository-family/alias grouping still needs review before a strong generalization claim.

Five packs freeze the exact existing base build scripts and source hashes. No real API/security contract has yet been independently grounded. A root script's presence is not proof of production correctness or PR intent; candidate violations require independent evaluation.

Offline preflight found:

| Pilot PR | Outcome before any model call |
| --- | --- |
| `muchu-dev/michinavi#46` | Complete prompt exceeds the frozen 64 KiB bound |
| `oxyhq/oxy#490` | No independently grounded root contract pack; it is a Bun workspace without the selected root `build` script |
| `troyrhodes02/sightline#88` | Complete prompt exceeds 64 KiB |
| `abhishek21lift-oss/619-erp-frontend#28` | Source snapshots exceed the 512 KiB supported bound |
| `dasch-swiss/dsp-app#3384` | Context fits, but the build contract has only null input and offers no additional hunter search space |
| `ronancodes/smart-cart#431` | Complete prompt exceeds 64 KiB |

This is a failed **configuration-feasibility gate**, not six measured model failures and not a rejection of counterexample hunting in general. A model choosing null input for an already-covered build would not test the thesis. The adapter rejects such a no-op before spending inference budget. CI rejects a comparison with zero eligible paired PRs after uploading its report.

All six PRs and both arms remain in the preflight report. There are zero live paired trials. Confirmed violations, wrong-block rates, review minutes, end-to-end latency and billed dollars are unmeasured. Zero observed candidates is not a measured zero-percent detection rate.

Raw evidence: `target/counterexample-validation/source-preflight.json`, `hunter-preflight.json`, and `pilot-preflight.json`.

## Inference and execution boundaries

`configuration.json` freezes one client/model configuration: the audited Claude Code 2.0.55 source, `claude-sonnet-4-6`, one turn, no thinking, 2,048 output tokens, a 120-second parent deadline, a $0.50 CLI spending limit and no retries. The CLI runs in a fresh empty directory with instruction-file loading disabled, no tools or MCP servers and no settings sources. Its source hash is pinned; changed clients, extra turns and model drift stop the configuration. CLI cost estimates remain distinct from actual billing.

No model invocation has verified provider availability or the complete startup behavior yet. Unit fixtures and inspection of the installed client establish the configured controls, not an observed live blind run. Instructions/model availability were checked against local source and official documentation; references are in `experiments/counterexample/README.md`.

PR execution is confined to Linux CI. A manually controlled experimental workflow builds the trusted worker, tests gVisor and runc, and retains raw observations. The current branch is configured to start that workflow when pushed. No model credential enters the job that runs PR code.

## Verification actually executed

Final local Rust commands used `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, and `KERN_CONTAINER_CLI=kern-validation-no-container`.

| Command | Actual result |
| --- | --- |
| `cargo test --locked --offline --test answer --test verdict --test check counterexample_validation --no-fail-fast` on the starting implementation | Exit 101; all eight new regressions failed |
| `cargo test --locked --offline --test verdict webpack_22695_parameterized_suites_do_not_repeat_file_scans_for_every_assertion` before/after indexing | Exit 101 at 30.01 seconds / exit 0 at 2.79 seconds |
| `cargo test --locked --offline --no-fail-fast` on the final Rust source | Exit 0; 619 passed, zero failed, one ignored |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Exit 0 |
| `python -m unittest discover -s experiments/counterexample -p 'test_*.py' -q` | Exit 0; 15 passed |
| `git -c core.whitespace=cr-at-eol diff --check` | Exit 0 |
| Experimental workflow YAML parse | Exit 0; four jobs parsed; Actions execution not verified |
| Original/pre-index full replay | Interrupted; both child processes exited -1; partial outputs retained |
| Indexed full replay | Failed the 300-second process deadline; child completion exit code unavailable; not a passed gate |
| Linux/gVisor/runc execution and live inference | Not run |

Container-dependent local tests can return early when containers are unavailable. The Rust pass count must not be interpreted as proof that Linux sandbox integration ran. CI remains required by repository policy.

## Compatibility and remaining limits

Receipts add explicit setup-comparability evidence. Older incomplete receipts cannot be rendered/re-signed through the stricter signer without a new run; existing signature verification is a separate path. Legacy seals without a markdown digest retain the existing compatibility behavior and are not made text-tamper-proof by this patch.

Receipt validation checks required shapes; it is not a complete typed model of every optional fact and does not prove the truth of worker observations. API analysis remains a limited static ESM surface analysis, not a general runtime or signature-compatibility verifier. Type confirmation remains unfinished. Normative adjudication and measured human review burden remain absent.

No production dependency was added, and no hosted service, required-check rollout, production twin-runtime redesign or remediation feature was introduced. The existing `one` and `main` references were not modified. Changes are local and uncommitted: deleting a branch alone would not discard uncommitted working-tree changes. Unrelated pre-existing untracked work was preserved.

## Decision and next evidence

Keep the trust fixes and measured indexing improvement for review. Do not claim the hunter works, ship it, or pay to run the current empty-search pilot.

Before a meaningful comparison, ground an API/security invariant from existing authoritative artifacts on development repositories, give the hunter a real approved input domain, and replace whole-file context packing with a bounded, provenance-preserving context selection strategy. Apply those changes uniformly and freeze them before inspecting holdout outcomes. Keep unsupported cases in the denominator. Then run the same deterministic oracle and resource ceilings on both arms, adjudicate violations/intent independently, and measure review burden, latency and cost.

The user replied “continue” to the explicit request to commit/push this isolated branch and run the capped Linux verification. That authorization covers this experimental branch on `jastijayakrishna/Kerner`, at most 12 runner-hours and six 120-second model calls. It does not authorize merging, release or local Docker. The results above are the local checkpoint before publishing; CI results will be recorded separately.
