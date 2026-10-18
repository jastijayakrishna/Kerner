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

`hunter.py --cli` uses the existing Claude login and pins the installed Claude Code 2.0.55 source to SHA-256 `97641f09bea7d318ce5172d536581bb1da49c99b132d90f71007a3bb0b942f57`. It uses `claude-sonnet-4-6`, one turn, no thinking, a $0.50 CLI spending limit, no built-in tools or MCP servers, no settings sources, a fresh empty working directory, and a 120-second parent deadline. The audited local source supports `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`; the adapter sets it so global/project instruction files do not enter the hunter context. The complete prompt is capped at 64 KiB, output at 2,048 tokens, and the pilot at six calls with no retries. A changed client, model drift, extra turn or over-limit cost stops the configuration. CLI-reported costs are estimates, not billing evidence. No real invocation has yet verified provider availability or the complete startup behavior.

`configuration.json` freezes the CLI backend for this pilot, including the instruction digest and client source digest. There is no API backend in this implementation. Switching backends or changing instructions requires a new configuration before any inference results are collected.

The [model identifier](https://platform.claude.com/docs/en/models/sonnet-4-6/overview) was checked against official documentation on 2026-10-18. Request/response fixtures test isolation and schema behavior, not provider availability. The CLI process has a 120-second parent deadline. Timed-out requests may still incur provider cost; missing billing evidence remains unknown.

Run `compare_replay.py` only for the separate repair audit. It compares identical archived inputs with the frozen original and repaired binaries. It cannot measure new static-rule coverage or live hunter lift.
