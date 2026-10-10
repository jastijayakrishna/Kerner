# Kern drive: run every changed line both ways — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** For any pull request, Kern counts which changed lines actually ran on the old and on the new code, runs an agent-written
driver in its gVisor sandbox to make them run, records the changed modules' calls itself, tells the agent which lines did not run,
and writes one PR comment: what ran, what behaves differently, what was not run.

**Architecture:** Two commands. `kern drive prepare` is static (any OS): it reads both commits, finds the changed lines that hold
code (oxc), finds the changed modules' exported functions, and writes a task folder for whoever writes the driver (the agent that
made the change, or an agent Kern's operator runs). `kern --control verifier.json drive run` is the sandbox half: it installs each
tree's dependencies once into a volume (any package manager, install scripts off, network only during install), installs Kern's
pinned driver tools (tsx, c8, jsdom) through the existing strict installer, then runs the driver twice on each tree in gVisor with the
network off, with line coverage and a call recorder, and scores the round. The agent loop stays outside Kern.

**Tech Stack:** Rust (oxc 0.152 for line and export analysis, existing sandbox/flow/git modules), Node 22 in the sandbox image, an
embedded ES module loader (tsx for TypeScript/JSX, an asset stub, Vite's import.meta.env, fixed time/randomness, a call recorder),
c8 for V8 line coverage through source maps.

Validated before building: the scratch experiment of 2026-10-09 (30 random agent PRs, Sonnet drivers, up to 3 rounds) reached
"at least 80% of changed lines ran both ways" on 25/30 PRs. This plan moves that harness into Kern.

---

## File structure

- Create `src/drive.rs` — everything specific to drive: `measured` (code lines), `changed` (changed lines per side), `exported`
  (exported functions), `prepare` (task folder), `setup`/`run` (sandbox rounds), `score` (report, feedback, comment).
- Create `src/drive-loader.mjs` — the in-sandbox loader (tsx, assets, import.meta.env, fixed world, call recorder).
- Create `src/drive-run.mjs` — the in-sandbox wrapper that imports the driver and always exits so coverage is written.
- Create `src/drive-task.md` — the driver contract the agent reads.
- Create `src/drive-tools/package.json`, `src/drive-tools/package-lock.json` — Kern's pinned tools (installed by the strict installer).
- Modify `src/compare.rs` — expose the Myers line diff as `changed_lines` (pure_removals keeps using it).
- Modify `src/sandbox.rs` — `Spec::workspace` (a volume at /workspace), the `/kern-tools` volume, entry points `__drive` and
  `__drive_setup`, readable copy for the setup.
- Modify `src/lib.rs`, `src/main.rs` — the module and the `drive prepare|run|clean` commands, help text.
- Create `tests/drive.rs` — unit tests; extend `tests/kern.rs` sandbox argument tests.
- Create `scripts/e2e-drive.sh` — the gVisor end to end check (a TS + JSX project, a driver, both rounds).

## Task 1: Changed lines and code lines (static)

**Files:** `src/compare.rs`, `src/drive.rs`, `tests/drive.rs`

- [ ] Test `changed_lines_on_each_side`: old `a\nb\nc\nd` → new `a\nB\nc\nd\ne` gives old `[2]`, new `[2, 5]` (1-based).
- [ ] Test `code_lines_skip_comments_imports_types_and_lone_brackets`: a TS file with a comment, an import, an interface, a
      multi-line parameter type, a function body and a lone `}` — only the body and the signature lines count.
- [ ] Test `exported_functions_are_found`: `export function a`, `export const b = () =>`, `const c = function(){}; export { c as d }`,
      `export default function`, `export const VALUE = 1` (not a function), `export class K` (not wrapped), CommonJS `module.exports = {..}`
      (marked cjs, names come at run time).
- [ ] Run `cargo test --test drive` → FAIL (no module). Implement `compare::changed_lines`, `drive::measured`, `drive::changed`,
      `drive::exported`. Run → PASS. `cargo test` stays green.

## Task 2: `kern drive prepare`

**Files:** `src/drive.rs`, `src/drive-task.md`, `src/main.rs`, `src/lib.rs`, `tests/drive.rs`

- [ ] Test `prepare_writes_the_task_for_a_commit_pair`: a temp git repo with a base and a head commit (TS module, a test file, a
      README); `drive::prepare(repo, base, head, out, Some(pr))` writes `drive.json` (files with old/new code lines; modules with
      exported functions; totals), `task/task.md`, `task/pr.md`, `task/diff.patch`, `task/lines.md` (every measured line with text),
      `task/old/` and `task/new/` (sources, no node_modules); test files and README are not measured.
- [ ] Implement with `git::read_commit`, `compare::probeable`, Task 1 functions; the diff via `git diff --no-renames`. CLI:
      `kern drive prepare <repo> <base> <head> <out-dir> [pr.md]`.

## Task 3: Sandbox support

**Files:** `src/sandbox.rs`, `tests/kern.rs`

- [ ] Test (extend the sandbox argument test): a `__drive` spec with `workspace: Some(("kern-deps-w", true))` and the tools volume
      mounts the volume read-only at /workspace, the tools read-only at /kern-tools, binds only /input and /kern, network none,
      user 1000; `__drive` with a bridge network, root, a writable workspace or no workspace volume is refused; `__drive_setup` must be
      root, bridge, writable workspace volume, no tools; tools at any other path or name is refused.
- [ ] Implement `Spec::workspace: Option<(&str, bool)>` (volume, readonly), allowed volume targets, the inspect checks in `execute`,
      `inside::drive()` (runs c8 + node + loader + run wrapper, prints sections after markers: output, errors, coverage, calls,
      behaviour) and `inside::drive_setup()` (readable copy of /input into the volume, then each install folder with its package
      manager, scripts off, frozen lockfile first, plain install as fallback; prints one JSON record).
- [ ] `cargo test`, `cargo clippy --all-targets` clean.

## Task 4: Loader, run wrapper, tools

**Files:** `src/drive-loader.mjs`, `src/drive-run.mjs`, `src/drive-tools/*`

- [ ] Loader: tsx from /kern-tools; resolve hook stubs assets, redirects every import of a changed ES module to a generated wrapper
      (`export *` from the real module plus a hoisted function per exported function that records the call and delegates); load hook
      serves wrappers and rewrites `import.meta.env`; CommonJS changed modules get their function exports wrapped after `require`;
      fixed Date/Math.random/crypto; the call log (first 100 calls per function, values written like probe.cjs) is written at exit.
- [ ] Run wrapper: imports /kern/driver.mjs, logs unhandled rejections, exits after 90 s or 300 ms after the driver finishes.
- [ ] Tools lockfile generated with npm (`npm install --package-lock-only tsx@4 c8@10 jsdom@26`), registry.npmjs.org, integrity pins.

## Task 5: `kern drive run` and the score

**Files:** `src/drive.rs`, `src/main.rs`, `tests/drive.rs`

- [ ] Test `score_counts_lines_both_ways_sets_noise_aside_and_reports_differences` (pure function on synthetic run outputs): coverage
      per side from c8 JSON, a step that differs between two runs of one side is noise, a step and a recorded call that differ between
      old and new are differences, behaviour lines new in the new runs are differences; `feedback.md` lists missed lines with text and
      the error tail; `comment.md` says "Kern ran N% of the changed lines on both the old and the new code".
- [ ] Implement `setup` (volumes per side via `__drive_setup`, tools via `flow::install_dependencies`, names kept in `setup.json`),
      `run` (control dir with loader, wrapper, driver, drive.json, tsconfig override; four runs, two at a time; parse sections;
      `rounds/N/report.json`; `task/feedback.md`; `comment.md`), `clean` (remove volumes). CLI:
      `kern --control verifier.json drive run <out-dir> <operator.json> <driver.mjs>` and `drive clean <out-dir>`.

## Task 6: End to end on gVisor

**Files:** `scripts/e2e-drive.sh`

- [ ] A project with a TS module (alias import, enum), a TSX component and a CommonJS helper; a change that alters one branch; a
      driver that reaches it. Expect: coverage 100% new and old, a step difference, a recorded call difference; a weak driver gets
      feedback naming the missed lines. Run in kern-vm with the static musl build.

## Task 7: Proof and field test

- [ ] Re-run the 30 experiment PRs through `kern drive` (same agent loop, Sonnet, up to 3 rounds): expect about 25/30 at ≥80%.
- [ ] Field test on agent-written PRs in well-known companies' public repositories; analyse failures; fix what generalizes; re-run.
