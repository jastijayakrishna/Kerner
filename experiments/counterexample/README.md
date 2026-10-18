# Disposable counterexample experiment

This is a validation branch, not a new production verdict path. The [protocol](../../docs/experiments/counterexample-validation.md) defines what counts as success and what remains outside scope.

The frozen cohort has 328 PRs from 99 repositories: 69 development PRs from 19 repositories and 259 holdout PRs from 80 repositories. `pilot.json` selects two development and four holdout repositories by a second fixed hash, then one PR per repository by a key hash. Selection did not use defect explanations, labels, code or run success. The larger cohort is a source pool; the six-PR pilot has its own complete denominator. Fork/alias grouping is still an evaluation prerequisite.

`cohort.json` and `pilot.json` contain revision identities only. Private evaluation labels and later defect explanations stay under ignored `target/counterexample-validation/evaluation/`. Do not give that directory or the source benchmark to the hunter. Its entire context is the approved contract pack, `HUNTER.md`, and a bounded base/head bundle. A fresh model invocation must have no access to evaluation files, tools or previous audit context.

An independently approved pack must pass `validation::pack` and pin the base-revision source establishing each oracle. A hunter response must pass `validation::proposals`; it can add inputs only. Fixed commands can run a project build or a narrow JSON-returning API/security probe. An exception, crash, malformed report or missing dependency does not become an API violation. Unsupported contracts remain unsupported. Contract source pins establish provenance; a person still has to approve that the oracle faithfully expresses the source and PR intent.

Place approved packs under `packs/<owner>__<repo>__<number>.json` and frozen responses under `proposals/` using the same name. Five build packs freeze the existing pre-change build scripts; the sixth has no root build script and uses Bun. These are base-only contract reviews, not execution results or confirmations of PR intent. API/security contracts and real model responses remain absent; `contract-coverage.json` records those gaps. The live pilot cannot establish detection value until the required responses exist and the worker has actually run. An empty-model response is a valid measured outcome only if the model was invoked and its provenance recorded; a missing response is an unsupported PR.

`counterexample-validation.yml` runs when this experimental branch is pushed, and also supports manual dispatch once available in Actions. It builds and verifies this branch, exercises both gVisor and runc, and runs the six selected PRs with at most two parallel jobs. It does not publish comments, merge, approve PRs or update required checks. Publishing therefore starts CI and needs the requested approval. The sum of configured job timeouts is below 12 runner-hours for one workflow run. No hosted Kern service or new production dependency is introduced.

Local checks:

```
python -m unittest discover -s experiments/counterexample -p 'test_*.py' -v
cargo test --locked --offline --no-fail-fast
cargo clippy --locked --offline --all-targets -- -D warnings
cargo check --locked --offline --example counterexample
```

Use `CARGO_BUILD_JOBS=4`. The Windows local verification disables container lookup with `KERN_CONTAINER_CLI=kern-validation-no-container`; Linux sandbox checks therefore remain separate. When comparing two source revisions, use separate target directories or disable incremental reuse. Do not mix the baseline library into the repaired worker.

Raw observations, timing, confirmation outcomes, source pins, model usage and cost are retained. `benchmark.py score` produces one row per selected PR and arm, including missing jobs. Human review minutes, wrong blocks and normative adjudication remain null until measured. Synthetic CI fixtures use `structural-fixture-no-model` and must never be counted as real-PR or AI results.

The frozen CLI configuration was rejected by its live development calibration. `hunter.py --cli` pins Claude Code 2.0.55 to SHA-256 `97641f09bea7d318ce5172d536581bb1da49c99b132d90f71007a3bb0b942f57` and requests `claude-sonnet-4-6`, one turn, no thinking, a $0.50 CLI estimate limit, no tools/MCP/settings sources, disabled instruction-file loading and a fresh empty working directory. Its complete prompt cap is 64 KiB, output cap 2,048 tokens, and parent deadline 120 seconds. These requested flags did not establish the required single-model execution: task-scoped transcripts from one 22.453-second invocation contained responses labeled Sonnet, Haiku and Opus. The result failed its configuration guard; no response or reproduction was accepted and no retry was made. See the [measured report](../../reports/counterexample-validation-2026-10-18.md).

`configuration.json` retains the rejected configuration, including instruction and client-source digests. There is no API backend in this implementation. A conforming stateless endpoint and credential reference are required before a new configuration can be frozen. The six original PRs remain unsupported by the pilot preflight. The separate development case also has disputed zero-range semantics; it is not a confirmed bug or a holdout result.

The [model identifier](https://platform.claude.com/docs/en/models/sonnet-4-6/overview) was checked against official documentation on 2026-10-18. Unit fixtures test caller controls and schema behavior; they do not prove complete client isolation. Rejected envelopes now retain whitelisted numeric/model diagnostics without result text or credentials. Estimates stay separate from the proposal's billed-cost field. The first rejected invocation's complete envelope was not retained, so its actual billing and total provider-request count remain unknown. No additional provider calls were made to repair logging.

Run `compare_replay.py` only for the separate repair audit. It compares identical archived inputs with the frozen original and repaired binaries. It cannot measure new static-rule coverage or live hunter lift.
