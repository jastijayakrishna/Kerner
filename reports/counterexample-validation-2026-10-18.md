# Counterexample validation: measured repairs, unvalidated hunter

Date: 2026-10-18. Maturity: Validate. Branch: `codex/counterexample-validation`. Starting commit: `479030dd39e464e0c8eb9cbb58b10c9ce3b802f5`.

**The repairs passed full Rust verification and both Linux end-to-end modes. The AI architecture remains unvalidated: the six-PR pilot had zero eligible paired trials, and an additional development CLI invocation failed the frozen model checks. Infrastructure success does not establish product success. No merge or production release occurred.**

## What was implemented

The experiment has a frozen protocol/ADR, repository-separated cohort and pilot, independently sourced build-contract packs, strict proposal validation, a sandbox worker, a bounded inference adapter and an all-PR scorer. The worker shares Kern's installer, sandbox and `suite::blockable`; experimental findings never update required checks or approve PRs. The deterministic core is shared and charged to both arms' budgets.

The following defects were repaired in the existing decision path:

| Defect | Resulting behavior |
| --- | --- |
| Empty or unknown evidence could rebuild as clear | Signing rejects missing mandatory evidence, unknown test statuses, missing result lists and missing comparability fields. Direct receipt construction treats unknown test status as unverified. |
| Passing tests erased unequal setup | Setup uncertainty survives verdict construction, folding and replay, including dependency resolution without a usable lockfile. Recorded uncertainty also survives a contradictory summary flag. Receipts suppress the green Tests line. |
| Workflow names inherited metadata exemptions | Executable workflows named `release.yml` or `funding.yml` remain reviewable. Existing non-executable funding/release metadata and image exemptions remain scoped separately. |
| Deleted public entry files disappeared from API analysis | Removed bindings are reported. A new surface that cannot be parsed is reported as unchecked rather than silently skipped. |
| Manifest-only export removal was missed | Changed package manifests participate in API analysis; removed export-map paths/conditions are visible. These are advisory findings, not proof that an intentional API change is wrong. |
| Approval omitted signed-text integrity validation | Approval invokes the existing markdown-digest check before success. The regression models a valid signature callback with altered signed text. |
| Type errors blocked after only one comparison | An enforced type-check failure without fresh confirmation becomes unverified. It remains visible; it does not become clear. Production type confirmation is not implemented in this milestone. |

The eight initial regressions failed on the starting implementation. During development, additional tests exposed oversized-output and off-domain-input gaps in the experimental verifier; both were fixed before any inference or PR execution.

The probe boundary now rejects oversized or unreadable results, malformed exit codes, crash/OOM evidence, wrong contract identities, modified oracles/commands/revisions/budgets, and inputs outside machine-enforced domains. Input-domain types are null-only, an approved nonempty string prefix, or explicitly approved JSON. The hunter cannot define its own domain or expected result. Final review reproduced an additional build-probe defect: high exit codes other than a small known signal list could become failures. A new regression failed with cargo exit 101; the repair treats every code from 128 through 255 as missing evidence, and the rebuilt targeted regression passed with exit 0. This is a conservative classification of ambiguous exits, not a claim that every such exit was caused by a signal.

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

This audit used the earlier repaired binaries. The later export-limit and setup-consistency refinements passed regression, full-suite and sandbox checks; a new complete historical replay of the final source was not performed. The full replay gate remains incomplete.

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

The source pool is frozen at 328 PRs from 99 repositories: 69 development PRs from 19 repositories and 259 holdout PRs from 80 repositories. Its canonical cohort digest is `c675348049ce3c40d3309a17cb8ed0cdddd13cc479dae2cd80ad01c1c2e5f2fb`.

The pilot selects two development and four holdout repositories by a second fixed hash and one PR per repository by a key hash. Selection did not depend on defect explanations, source outcomes or run success. Future fixes/reverts and labels are stored separately from hunter inputs. None of these six cases reached inference. The additional development attempt below is separate. Repository-family/alias grouping still needs review before a strong generalization claim.

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

The real-PR worker requires both a contract pack and a frozen hunter response before starting a paired trial. Therefore baseline execution was also skipped. This is not evidence that Kern cannot execute those repositories or their five build commands. A future study should separate baseline execution from hunter eligibility when measuring baseline support. Running baseline-only work now would not rescue this configuration's missing AI search domain.

All six PRs and both arms remain in the preflight report. There are zero live paired trials. Confirmed violations, wrong-block rates, review minutes, end-to-end latency and billed dollars are unmeasured. Zero observed candidates is not a measured zero-percent detection rate. The actual CLI adapter entry point was also executed through preflight and retained six unsupported statuses; no request reached inference. Exit zero here means that failure statuses were recorded, not that validation succeeded.

The scorer reports core and probe review-item counts separately and combines them only when both are available. It does not mistake new probe findings for the complete review burden. Review minutes remain unknown. A compact, durable evidence snapshot is in `reports/counterexample-validation-evidence-2026-10-18.json`; the larger raw artifacts remain under ignored `target/` with their hashes recorded in that snapshot.

Raw evidence: `target/counterexample-validation/source-preflight.json`, `hunter-preflight.json`, and `pilot-preflight.json`.

## Inference and execution boundaries

`configuration.json` freezes the requested client/model controls: pinned Claude Code 2.0.55 source, `claude-sonnet-4-6`, one turn, no thinking, 2,048 output tokens, a 120-second parent deadline, a $0.50 CLI estimate limit and no retries. The caller uses a fresh empty directory and requests disabled instruction loading, tools, MCP and settings sources. These flags are not proof that the complete client obeys the intended one-model boundary. The live check below rejected that configuration.

### Additional development calibration: rejected

For `andyshinn/meshcore-ts#15`, I froze a documented decoder wire/API contract at base `2f419322af285801637c4c40244ae5141fc5f19b` before reading head implementation `2be2596acdc8bf55f225eda2a5eb0c79967d95e7`. The fixed pack digest is `f415cb2df9fcffc63be03563ff906a5434d0682cbc119d49e7998496c9c44341`. Its complete changed-file prompt was 15,408 bytes. This was a development-only feasibility calibration, not a replacement pilot or holdout result.

One bounded CLI invocation completed in 22.453 seconds and failed its model/turn/error guard. No response was accepted, and no reproduction was executed. A scoped read of this invocation's matching transcript directory found responses labeled `claude-sonnet-4-6`, `claude-haiku-4-5-20251001` and `claude-opus-4-5-20251101`; the latter two were marked sidechains. No tool use appeared in those matching assistant messages. Their purpose and the precise failing envelope field were not established. These records invalidate a single-model claim; they do not prove the absence of other requests or validate complete startup isolation. There was no retry or fallback. Actual billing and total provider-request count remain unknown.

The head also intentionally introduces zero-valued frequency bounds as end-of-list sentinels. The base comment alone does not establish that zero-valued ranges must remain valid. The calibration's zero-range expectation therefore needs independent policy/intent adjudication. A historical mismatch here is not labeled a confirmed product violation.

The first failure path discarded its envelope before saving usage. I reproduced and repaired that measurement defect with a negative fixture, preserving only whitelisted numeric/model diagnostics for future rejected calls. A second regression prevents CLI dollar estimates from being promoted into the proposal's billed-cost field. Both tests failed before repair; the final Python suite passes 18 tests. This does not recover this attempt's unknown billed cost or make its response acceptable. The model configuration remains stopped, pending an approved stateless credential/endpoint and a defensible contract domain.

PR execution is confined to Linux CI. A manually controlled experimental workflow builds the trusted worker, tests gVisor and runc, and retains raw observations. The current branch is configured to start that workflow when pushed. No model credential enters the job that runs PR code.

## Observed sandbox mechanics

The synthetic regression and equivalent-edit control completed in both runc and gVisor on the published code. Both runtimes produced the expected results:

| Supplied fixture | Baseline confirmed candidates | Arm with supplied counterexample |
| --- | --- | --- |
| Parent-path policy regression | 0 | 1 |
| Equivalent implementation edit | 0 | 0 |

All eight final-source arm runs were within budget with no recorded harness errors. Arm durations, including the charged core check, ranged from 6.47 to 15.02 seconds. These were **trusted synthetic inputs, without a model call**. They demonstrate fresh-run confirmation and control behavior; they do not establish real-PR detection value, a wrong-block rate or AI superiority.

The first CI run was cancelled to separate an opaque combined step; its logs showed steady end-to-end progress, not a timeout. Later superseded runs were stopped as regression-driven safeguards changed. The completed Rust-source run is [39141556731](https://github.com/jastijayakrishna/Kerner/actions/runs/39141556731), on `74df39645784c59acd28dbf2a8b68d8891912c09`. Both full end-to-end jobs passed. Its qualification job correctly failed with exit 2 because eligible paired real-PR trials equaled zero. All six unsupported PR artifacts and the qualification report were retained.

The final fixture and qualification ZIP hashes match GitHub's artifact digests. Across this experiment's five runs, GitHub job timestamp intervals sum to 6,536 seconds (108.93 minutes), including superseded runs. This is below the authorized 12 runner-hours; it is not an invoice-dollar measurement. GitHub's timing endpoint reported zero billable milliseconds, which is not used as a claim of zero infrastructure cost. Raw timing summaries are in the adjacent `counterexample-validation-job-time-2026-10-18.json` and `counterexample-validation-ci-usage-2026-10-18.json`.

## Verification actually executed

Final local Rust commands used `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, and `KERN_CONTAINER_CLI=kern-validation-no-container`.

| Command | Actual result |
| --- | --- |
| `cargo test --locked --offline --test answer --test verdict --test check counterexample_validation --no-fail-fast` on the starting implementation | Exit 101; all eight new regressions failed |
| `cargo test --locked --offline --test verdict webpack_22695_parameterized_suites_do_not_repeat_file_scans_for_every_assertion` before/after indexing | Exit 101 at 30.01 seconds / exit 0 at 2.79 seconds |
| `cargo test --locked --offline --no-fail-fast` on the final Rust source | Exit 0; 620 passed, zero failed, one ignored |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Exit 0 |
| `python -m unittest discover -s experiments/counterexample -p 'test_*.py' -q` | Exit 0; 18 passed locally, including the later failure/cost measurement fixes; the CI revision ran 16 tests |
| Linux CI `cargo test --locked --no-fail-fast` at `74df396` | Exit 0; 625 passed, zero failed, one ignored |
| Linux CI `cargo clippy --locked --all-targets -- -D warnings` at `74df396` | Exit 0 |
| Linux CI musl release build of `kern` and `counterexample` at `74df396` | Exit 0 |
| `git -c core.whitespace=cr-at-eol diff --check` | Exit 0 |
| `C:\Program Files\Git\bin\bash.exe -n scripts/e2e-check.sh` after the progress-message edit | Exit 0 outside the restricted Windows child-process environment; parse only, no script commands executed |
| Experimental workflow YAML parse | Exit 0; four jobs parsed; Actions execution subsequently verified |
| Original/pre-index full replay | Interrupted; both child processes exited -1; partial outputs retained |
| Indexed full replay | Failed the 300-second process deadline; child completion exit code unavailable; not a passed gate |
| `E2E_RUNTIME=runc KERN_BIN=$PWD/bin/kern sh scripts/e2e-check.sh` in CI | Exit 0; `e2e-check: PASSED` |
| `E2E_RUNTIME=runsc KERN_BIN=$PWD/bin/kern sh scripts/e2e-check.sh` in CI | Exit 0; `e2e-check: PASSED` |
| Real-PR qualification gate | Exit 2; zero eligible paired trials; intended no-go result |
| Development CLI invocation | Adapter recorded unsupported after 22.453 seconds; no accepted trial |

Container-dependent local tests can return early when containers are unavailable. Linux integration is supported by the separate completed CI jobs, not inferred from the Windows test count. macOS was not executed. Later operator-client diagnostic changes were verified by 18 local Python tests; Rust source remains byte-identical to `74df396`. The E2E script's stale type-check heading was corrected after CI without changing its assertions or commands.

## Compatibility and remaining limits

Receipts add explicit setup-comparability evidence. Older incomplete receipts cannot be rendered/re-signed through the stricter signer without a new run; existing signature verification is a separate path. Legacy seals without a markdown digest retain the existing compatibility behavior and are not made text-tamper-proof by this patch.

Receipt validation checks required shapes; it is not a complete typed model of every optional fact and does not prove the truth of worker observations. API analysis remains a limited static ESM surface analysis, not a general runtime or signature-compatibility verifier. Export-map nesting beyond 16 levels now becomes explicitly unchecked; the boundary regression failed before the repair. Type confirmation remains unfinished. Normative adjudication and measured human review burden remain absent. Some preparatory subprocesses capture output before checking its size; the prototype's byte thresholds are not a complete host-memory isolation proof.

No production dependency was added, and no hosted service, required-check rollout, production twin-runtime redesign or remediation feature was introduced. The existing `one` and `main` references were not modified. The work is isolated on `codex/counterexample-validation`; unrelated pre-existing untracked work was preserved. No merge or production release occurred.

## Decision and next evidence

Keep the trust fixes and measured indexing improvement for review. Do not ship the hunter or claim it outperforms the baseline. The complete replay gate and product-evidence gate have not passed.

Before another model trial, use an approved stateless endpoint and verify its actual model identity, tool access, request count and usage. Resolve normative input domains independently; base comments and previously accepted outputs do not settle disputed requirements. Run deterministic contract checks first, then spend hunter budget on remaining searchable contracts. Select eligible repositories using pre-outcome criteria, preserve unsupported cases, and freeze context selection before holdout evaluation. Decouple baseline execution from hunter eligibility. Then run shared infrastructure/budgets, independently adjudicate violations and intent, and measure review time and cost. The credential reference requested during this task is still missing.

The user replied “continue” to the explicit request to commit/push this isolated branch and run capped Linux verification. Only this experimental branch on `jastijayakrishna/Kerner` was published. No local Docker was run, and unrelated pre-existing work was preserved. The configuration stopped after its first CLI invocation; the credential question remains unresolved and no retry was made. The later diagnostic/report follow-up is separately identified from the completed Rust CI revision.
