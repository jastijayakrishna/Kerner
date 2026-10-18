# Counterexample validation v1

Status: Validate. Isolated branch: `codex/counterexample-validation`. Starting commit: `479030dd39e464e0c8eb9cbb58b10c9ce3b802f5`. No production rollout or signing changes to accept experimental findings.

The outcome is evidence for a decision: does one contract-directed counterexample hunter find additional reproducible violations worth its latency, cost and review burden? Passing engineering tests alone cannot answer this.

## Frozen comparison

Compare the repaired deterministic baseline with the same baseline plus one hunter configuration. Both arms use the same Kern installer, pinned image, isolated test runner, contract pack, confirmation rule and resource ceilings. The frozen original commit is also replayed as a separate repair audit, not counted as hunter improvement.

Each arm has 1,800 seconds per PR, including installation, with at most 120 seconds per probe process. Two fresh installations of each revision are required. Eight baseline inputs and eight hunter inputs maximum per contract; four contracts maximum per PR. Installation uses Kern's existing restricted network policy; probe execution has no network, credentials or host writes. No Docker execution on this workstation.

The repaired existing Kern check runs once as a shared deterministic core with `KERN_CHECK_MINUTES=10` and no AI. Its elapsed cost is charged to both arms' ceilings and its receipt is retained. The two arms add contract probes to that identical evidence; neither may remove a core finding. Arm order is counterbalanced by the head commit. Budget overruns invalidate confirmation. The present installer can overshoot its deadline while stopping a process; such a run is retained as an overrun, never counted as a successful budgeted run.

A contract pack is trusted evaluation infrastructure, approved independently of the proposed change and hunter. It names the exact base and head, the pre-change source path and SHA-256 supporting each contract, a fixed probe command, a fixed oracle, and deterministic baseline inputs. Supported oracle modes are successful build exit and exact JSON output. They can express narrow API and explicit security rules. A changed output without an independent contract is not a confirmed product bug. Missing contracts, unsupported tooling and exhausted budgets remain in the denominator.

Input domains are machine enforced: null-only, strings beginning with an approved nonempty prefix, or explicitly approved arbitrary JSON. A prose-only domain is rejected. Both baseline and hunter inputs must satisfy the same domain before any execution. The current five real build contracts are null-only; they provide no meaningful additional input search. API/security feasibility on real PRs is therefore still unresolved, even if structural fixtures pass.

The hunter receives only base/head snapshots, their diff, and approved contracts. It returns JSON inputs referencing existing contract IDs. It cannot supply commands, assertions, expected outputs, revisions, budgets or verdicts. One response, at most eight additional inputs per contract; no adaptive retry. Model identity, prompt digest, response digest, token usage and billed cost must be recorded. An unknown cost is null, never zero.

Freeze dataset membership and repository split before exposing holdout source. Split by SHA-256 of `kern-counterexample-v1/` plus the lowercase repository; first byte modulo five equals zero for development, otherwise holdout. Repository aliases and forks require group adjudication before a valid holdout claim. PR titles and descriptions are untrusted data. Later fixes, reverts, defect explanations and labels are evaluation-only; never include them in model context or command arguments. An absence of a revert is a control label, not proof of correctness.

## Measures and decision

One row per selected PR and arm, including missing runs. Record supported contracts, independently confirmed violations, newly confirmed violations over baseline, wrong blocks, disputed intent, review items, measured review minutes, unsupported reasons, wall seconds, model tokens and billed dollars. Report all-PR and supported-only rates, separate development/holdout results, and paired discordant cases. Never count a synthetic fixture as a real PR.

Continue to a larger validation only with at least one additional independently adjudicated holdout violation, no known wrong block, and complete rows for the frozen cohort. This is a pilot decision, not a production safety threshold. Production blocking remains disabled until a larger study bounds wrong blocks and proves acceptable review burden and cost. No minimum PR count can establish universal correctness.

Missing runtime, credentials, approval to publish the branch, approved contracts, source snapshots, model configuration, cost or adjudication is reported explicitly. Missing requirements are not fabricated from a later fix. Do not silently reduce the cohort to runnable or reverted PRs.

## ADR: proposal and oracle separation

The hunter chooses inputs; trusted contracts and Kern decide whether observations violate them. Reuse `suite::verdict`, `suite::blockable`, the installer and sandbox. Keep this experiment outside the required-check workflow. A model judging its own generated assertion, running PR code beside model credentials, treating reverts as certain defects, or replacing the complete denominator with completed checks would invalidate the experiment.

The separate contract authoring step costs effort and limits coverage. That is an observable product limitation. Automatically inferring normative requirements is deferred. Type failures without fresh confirmation are demoted, not given a fabricated confirmation bit. Production twin-runtime redesign, remediation, hosted services and general correctness claims are outside this milestone.

The first CLI calibration failed its single-model boundary; its outputs are excluded. Caller flags and a client-source pin do not establish the absence of auxiliary inference. The frozen configuration is retained as failed evidence, not relaxed to accept the observed extra models. The documented decoder calibration also exposed disputed input validity around zero-valued sentinels. Resolve the oracle's authority/domain before treating a difference as a bug.

## Verification and stopping

Run regression tests before repairs, then full Rust tests and clippy with four build jobs. Compile the experimental worker and verify negative proposal/oracle cases. Run sandbox integration in Linux CI in both modes required by repository policy. Keep the cohort, protocol, contracts, proposals, raw observations and evaluation separately hashed. Stop expansion on a confirmed wrong block, an oracle change after holdout exposure, leakage, missing isolation, or a budget overrun; retain failed rows and artifacts.

The experiment owner is the repository maintainer. Review immediately after the frozen run. Expand, revise or delete this branch based on the paired evidence; do not merge it merely because its tests pass.
