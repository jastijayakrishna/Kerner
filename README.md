# Kern

**Kern checks code that AI agents write, before it can hurt your customers.**

AI coding agents such as Claude Code, Codex, GitHub Copilot, Devin and Cursor now write many pull requests. They are fast. But a reviewer cannot read every line, and tests only check what someone thought to test. Kern does the checking for you, and shows you in plain words what each change really does.

## The problem

A change from an AI agent can look fine and still:

- **touch what it should not**: CI settings, secret files, dependencies;
- **add risky powers**: start programs, call the internet, read secret keys;
- **cheat its tests**: delete, skip or fake a test so that it passes;
- **change more than it was asked to**: remove a safety check, charge a customer twice;
- **break real customers** on inputs that no test ever tried.

## Add Kern to a repository: one file

Save this file in your repository as `.github/workflows/kern.yml` ([examples/kern.yml](examples/kern.yml)):

```yaml
name: Kern
on:
  pull_request:
  issue_comment:
    types: [created]
permissions:
  contents: read
  pull-requests: write
  issues: write
  statuses: write
  id-token: write
jobs:
  kern:
    if: github.event_name == 'pull_request' || (github.event.issue.pull_request && startsWith(github.event.comment.body, '/kern'))
    uses: jastijayakrishna/Kerner/.github/workflows/kern.yml@main
    secrets: inherit
```

That is the whole install. There is no sign-up and no Kern server: Kern runs on your repository's own GitHub Actions runners, and
public repositories pay nothing for them. Keep the `permissions` as they are: Kern's jobs ask for exactly these, and GitHub does not
start them with less (`id-token: write` is how GitHub signs each receipt).

Two optional settings:

- **Require the `Kern` status** (Settings, then Branches), so nothing merges without its receipt.
- **Add an AI key** as an Actions secret, `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` (from `claude setup-token`). Kern then also
  runs every changed line on the old and the new code. Without a key, your code never leaves GitHub's runners.

See it on real pull requests: [kern-check-demo](https://github.com/jastijayakrishna/kern-check-demo/pulls?q=is%3Apr).

## What every pull request gets: one receipt

Kern posts one comment, the receipt, and edits it on every push. It sets one status, `Kern`. The receipt starts with the answer:

| Answer | What it means | What you do |
|---|---|---|
| ✅ **Nothing unexpected** | Every test that passed before still passes, and nothing below was found | Merge as usual |
| ⚠️ **Needs a look** | Something changed that the pull request may not mean | Look. If each item is meant, someone who can merge comments `/kern approve` |
| ❌ **Blocked** | Something that worked before is broken, or the tests were made to pass instead of the code | Fix it. An approval cannot clear a block |

For example, from a real run on the demo repository:

```text
⚠️ Kern: 1 thing needs a look
- src/version.js can now do something it could not before: node:child_process
✅ Tests: 12 that passed before still pass (vitest)
✅ No skipped, focused or removed tests
```

What Kern checks on every pull request:

1. **Your own tests, on the old and the new code.** Kern installs both versions and runs the project's tests the way its own `test`
   script does (the runner, its config, the test files and the variables it sets; in a monorepo whose script hands the tests to each
   package, each changed package's own script), on the Node version the project asks for (`.nvmrc`, `.node-version`, `.tool-versions`,
   `volta`, `engines.node` or its CI), with the network off while the tests run. A test that passed before and fails now
   blocks, but only when it fails again when Kern runs its file alone on the new code, and not when the old code runs that file alone.
   A failure that shows only when the whole suite runs needs a look; one that does not repeat is called flaky. A test that stops running,
   or is newly skipped, needs a look, checked the same way. Tests that already failed before the change are set aside, and so is a new
   test that fails here with the same error as they do (the sandbox lacks something, not the change). A block needs proof from both
   sides: a test file with no record of passing on the old code, or a failure on sides Kern could not set up the same way (installs,
   Linux parts, a build step), needs a look instead. A project setting that stops
   at the first failure or shuffles the order is turned off, so both sides run the whole suite in the same order. The old code's tests
   run first: when they cannot load because a package of the same repository was never built, or because the steps the test script
   names before its runner (`npm run build &&`, a `pre` script) did not run, Kern runs that build script or those steps as the test user
   without a network, makes the result read-only, and always does the same on the new code; when they still cannot run, Kern says why
   at once instead of running the new code too. A suite on course to take more than twice its time limit is stopped after a few minutes,
   and a suite too big for the limit is checked with the tests related to the change. When the change
   edits, or moves and edits, an existing test file, its snapshot, or a data file or helper the test names, Kern also runs the old
   test, with its old data, on the new code (jest, vitest and mocha): a test whose old version now fails needs a look, because the
   change rewrote what it expected. So does an edited test file whose old version Kern cannot run there (node's test runner, for
   one), and an edited data file, helper or mock that no old test names.
2. **Whether the tests check the change at all.** When the tests pass, Kern plants a few small faults in the lines the change touched
   (`>` turned into `>=`, a condition turned around, a call left out), one at a time, and runs the tests related to each file (jest and
   vitest). The fault is put in as the file is read, so nothing on disk is edited. A fault that makes a test fail that passed without it
   is caught. The receipt says how many were caught and names the lines where none was, and when no test notices any of them the change
   needs a look: the tests pass, but nothing in them checks this change. A fault that only puts back the old line, logging calls, and a
   run in which the fault never reached the code are not counted.
3. **TypeScript errors.** For each TypeScript project the change touches, Kern runs the project's own `tsc` on the old and the new
   code, in the sandbox. It compares them only when the old code type-checks with no error at all there, so a missing generated file
   cannot make anything look new. New errors block when the project's own scripts run `tsc` (and the project has a lockfile, so both
   sides got the same dependencies), and need a look otherwise. This works even when the tests cannot run.
4. **Test tricks.** An existing test file that newly skips, focuses or inverts tests (`.only`, `.skip`, `.skipIf`, `.fails`, `this.skip()`, under any
   name or chain), or code that reaches into the test runner,
   blocks. In a test file the change adds, it needs a look, since it cannot hide a test that passed before.
5. **New powers.** Changed code that newly starts programs, reaches the network or runs `eval`, tests included, and changed code outside
   the tests that newly reads environment variables, needs a look. Ordinary imports, such as a UI library or a project's own modules,
   are not powers.
6. **Files that steer everything else.** CI, secret and configuration files (notes such as Markdown files and `.env.example`
   templates aside), test runner settings (the test script in `package.json` included), and the files that instruct coding agents
   (`AGENTS.md`, `CLAUDE.md`, `.cursorrules`) need a look.
7. **Dependencies.** The receipt names what moved (`vitest 3.2.4 → 4.0.18`). An update within the same major version to a release
   that has been on npm for a week needs no look; a release less than a week old is named, because that is where hijacked packages
   show up. A new package, a major update and a lockfile entry that downloads a package from somewhere other than its own registry
   tarball need a look.
8. **Changes that break users without failing a test.** These need no test run, so they also cover projects whose tests Kern cannot
   run: tests removed from an existing test file, left checking nothing, or loosened to checks that pass for almost any value; a
   published package that stops exporting a name its users import; code that starts using a Node feature newer than the oldest Node
   the package says it supports (`engines.node`, checked against Node's own documentation); and HTML that adds inline styles or scripts
   that a Content-Security-Policy in the same change forbids.
9. **Behaviour, with an AI key.** Claude Code, in a locked-down mode, writes a small program that makes every changed line run. Kern runs
   it on the old and the new code and lists what behaves differently, with what the pull request does not mention first.

Where the code runs: Kern's workflow has two jobs. The job that installs and tests the pull request's code holds no keys at all (its
token can only read the repository) and runs on GitHub's throwaway machine, deleted when the job ends. There the code runs in locked
containers (read-only system, no privileges, an unprivileged user, memory and process limits, the network off while tests run), and
packages install with their own install scripts, the way CI installs them; when a script fails, Kern installs again without scripts.
A second job never runs the pull request's code: it builds Kern itself from source with no cache, renders the receipt from the first
job's results, signs it and posts it. On a machine that is kept between jobs, or when an AI key is set, the code runs in gVisor instead.

GitHub signs each receipt for Kern's own workflow, with no keys for you to keep. The signature covers the answer and the whole receipt,
so an edited comment cannot change what `kern receipt verify owner/repo 12` reports, and `/kern approve` only counts signed receipts.

Read more: [docs/drive.md](docs/drive.md) for the behaviour runs, [docs/merge-gate.md](docs/merge-gate.md) for the rules behind them.

## For agent pipelines: the merge gate

When an agent works on a task with written limits (which folders, which packages, which powers), the merge gate checks the change
against them in a sealed sandbox, and a separate verifier signs the result. GitHub refuses the merge without a valid proof, and an
audit re-checks every change merged in a period. Read more: [docs/merge-gate.md](docs/merge-gate.md)

## In production: the twin

```text
customer request ──▶ old version ──▶ answer to the customer (as today)
        │
        └── copy ──▶ old ghost ──┐
                     new ghost ──┴──▶ compare answers and writes ──▶ evidence

enough clean comparisons ──▶ new version serves ──▶ first surprise ──▶ old version serves again, before the next request
```

- A **ghost** is a sealed copy of one version of your service. It answers a copy of a real request, but it can touch nothing real: its writes are written down, never done.
- Both ghosts see the same clock and the same random numbers, so only real differences show.
- The new version is switched on only after enough clean comparisons. For example, 3,000 clean comparisons show, with 95% confidence, that fewer than 1 in 1,000 requests would differ. You can also ask for a minimum time, such as 24 hours.
- Each difference shows the exact request, the old answer and the new answer, and can name the pull request that caused it.
- A difference becomes a test case, with private data replaced, that your coding agent can fix.

The twin works with Node.js services today.

Read more: [twin/README.md](twin/README.md)

## Proof on real code

| What we tested | Result |
|---|---|
| Infisical pull request #6995, which removed a safety check | The twin caught it on real requests, with 0 false alarms |
| firecrawl pull request #2638, which only added a log line | The twin stayed silent, and saw all 22 changed lines run |
| 66 real pull requests written by AI agents | `kern drive` ran at least 80% of the changed lines in 55 of them (83%) |
| 55 attempts to cheat a real test suite | 0 accepted |
| 92 known tricks to hide new powers | 92 blocked, while 28 of 28 normal edits passed |
| A change that stopped `/admin` from checking the user's token | Blocked: "401 before, 200 now" |

Every result, with its date and how it was measured: [docs/evidence.md](docs/evidence.md)

## Try it

Kern is early, and there is no release yet, so build it from source. You need:

- Rust, to build `kern`;
- Node.js 22.3 or newer, for the twin and for `kern drive`;
- Linux with Docker, plus gVisor on a machine that is kept between checks (on Windows, use WSL2).

```sh
git clone https://github.com/jastijayakrishna/Kerner.git
cd Kerner
cargo build --release --locked
./target/release/kern --help
```

Watch the twin catch a planted bug in a small shop app (needs Docker):

```sh
sh twin/examples/shop/run.sh
```

Next steps:

- Put the merge gate on a repository: [docs/merge-gate.md](docs/merge-gate.md#quick-start-about-10-minutes)
- Comment on every pull request with `kern drive`: [docs/drive.md](docs/drive.md)
- Put the twin in front of a Node.js service: [twin/README.md](twin/README.md#use)

## What Kern does not promise

- Kern does not prove that code is correct or well meant. A person still decides whether a change is good.
- Kern tries many inputs, but not every input. Behaviour behind an input that nobody tried can still change unseen.
- Kern runs JavaScript and TypeScript today: the tests it runs are vitest, jest, mocha and `node --test`. For other projects the receipt
  covers the rules only and says so; changed code in other languages is listed as "not checked", never passed quietly.
- Kern runs Node 18, 20, 22, 24 or 26, the major the project asks for, from the official images; a project that asks for another major
  runs on the next one Kern has. It runs a build step or `pretest` script only when the tests cannot load without it. Tests that still
  cannot load cannot be checked, and the receipt says so with the first error instead of claiming anything.
- Kern runs the tests with the network off. Tests that need the network, a database or a build step first fail before and after the
  change, so they are set aside, never blamed on it, and the receipt quotes why they did not run.
- A pull request from a fork gets its receipt in the job's summary only: GitHub gives such runs a read-only token and no signature, so
  Kern cannot comment or set its status there. The job fails unless the answer is ✅. If you require the `Kern` status, merge such a
  pull request after reading that summary.
- Kern reads the test runner's own report. Code running inside the tests shares the runner's process and could fake that report, as it
  could in any CI; Kern flags new test-runner settings and hooks, and the job that signs never runs that code.
- A check has 55 minutes. Tests Kern had no time for are listed on the receipt as not run, never as passed or failed.

Every known limit: [docs/limits.md](docs/limits.md). Security model: [SECURITY.md](SECURITY.md).

## All documents

| Document | What it covers |
|---|---|
| [docs/merge-gate.md](docs/merge-gate.md) | Setup, task contracts, the test proof, the old-versus-new comparison, the local sandbox, the GitHub check, audits |
| [docs/drive.md](docs/drive.md) | `kern drive` and its pull request comment |
| [twin/README.md](twin/README.md) | The twin: setup, switching on and back, real databases, the fix loop |
| [docs/evidence.md](docs/evidence.md) | Every measured result |
| [docs/limits.md](docs/limits.md) | What Kern does not do yet |
| [SECURITY.md](SECURITY.md) | The trust model, and how to report a security problem |
