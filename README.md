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

One optional setting:

- **Require the `Kern` status** (Settings, then Branches), so nothing merges without its receipt. A pull request can also edit
  `.github/workflows/` itself, to skip Kern or set the status on its own, so the status only binds when workflow files cannot change
  without you: make Kern a required workflow in a repository ruleset, or add `/.github/workflows/ @your-team` to `CODEOWNERS` and require
  code-owner review. `kern receipt verify` accepts only receipts signed by Kern's own workflow at `main`, a release tag, or `one`
  while Kern One is built there.

Kern needs no AI key. With an `ANTHROPIC_API_KEY` secret it also investigates each pull request with Claude (item 10 below), in a job
of its own that reads the code and never runs it; the job that runs the pull request's code still holds no keys. That job sends
Anthropic the changed functions' old and new code, the old tests, documentation and callers that mention them, and the pull request's
title and body. Without the key, your code never leaves GitHub's runners. Pull requests from forks get no secrets, so no AI step.

See it on real pull requests: [kern-check-demo](https://github.com/jastijayakrishna/kern-check-demo/pulls?q=is%3Apr).

## What every pull request gets: one receipt

Kern posts one comment, the receipt, and edits it on every push. It sets one status, `Kern`. The receipt starts with the answer:

| Answer | What it means | What you do |
|---|---|---|
| ✅ **Nothing unexpected** | Every test that passed before still passes, and nothing below was found | Merge as usual |
| ⏭️ **Not verified** | Kern found nothing that needs a look, but it could not check part of the change: the tests could not run here, no test reaches the changed code, or a test failed in a way Kern could not prove or rule out. The receipt names each part and why | Decide whether that is acceptable. If it is, someone who can merge comments `/kern approve`. The status is an error, never green |
| ⚠️ **Needs a look** | Something changed that the pull request may not mean | Look. If each item is meant, someone who can merge comments `/kern approve` |
| ❌ **Blocked** | Something that worked before is broken, or the tests were made to pass instead of the code | Fix it. An approval cannot clear a block |

A look always names what Kern found: code that runs, access that is granted, tests that check less, or something hidden. What Kern
could not check is never a look; it is listed under **Not verified**. A change the pull request makes on purpose to a value an
existing test expects is listed under **Notes**, with the value before and after, so a person can scan it in seconds.

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
   package, each changed package's own script), on the Node version the project asks for (the CI workflow that runs its tests, the
   version file that workflow reads, `.nvmrc`, `.node-version`, `.tool-versions`, `volta` or `engines.node`), with the package manager it
   pins (through corepack, which checks the `packageManager` hash; pnpm 12 and later through Kern's own pinned corepack, because the one
   in older Node images cannot start them; a project that pins nothing gets corepack's own fixed version, never whatever is newest that
   day; the tests and build steps can call it), with the network off while the tests run. A test that passed before and fails now
   blocks only when it fails in every run of the new code that included it (the whole suite, its file alone, the whole suite again) and
   passes in every run of the old code, including a second one. A failure that shows only when the whole suite runs, that does not repeat
   in every run, that sits in a file whose other tests passed and failed by chance, or that comes from the sandbox (no network, a missing
   program or native module, a crashed process) is not verified: Kern says it cannot prove or rule out that the change broke it. One
   that never repeats is called flaky and set aside. A test that stops running, or is newly skipped, needs a look, checked the same way.
   Tests that already failed before the change are set aside, and so is a new test that fails here with the same error as they do (the
   sandbox lacks something, not the change). A block needs proof from both sides: a test file with no record of passing on the old
   code, or a failure on sides Kern could not set up the same way (installs, Linux parts, a build step), is not verified instead. A project setting that stops
   at the first failure or shuffles the order is turned off, so both sides run the whole suite in the same order. The old code's tests
   run first: when they cannot load because a package of the same repository was never built, or because the steps the test script
   names before its runner (`npm run build &&`, a `pre` script) did not run, Kern runs that build script or those steps as the test user
   without a network, makes the result read-only, and always does the same on the new code; when they still cannot run, Kern says why
   at once instead of running the new code too. A suite on course to take more than twice its time limit is stopped after a few minutes,
   and a suite too big for the limit is checked with the tests related to the change. When the change
   edits, or moves and edits, an existing test file, its snapshot, or a data file or helper the test names, Kern also runs the old
   test, with its old data, on the new code (jest, vitest and mocha), and reads how the old version fails, with the values the old
   test expected and the new code gave. When the old test expected the code to refuse something (to throw, to return an error status,
   not to call something) and the new code accepts it, or a yes or no, a something or nothing, or the shape of the result changed, it
   needs a look. When only a value moved (a string or a number, everything else the same), it is listed under Notes with both values,
   unless the pull request says it changes no behaviour, which makes it a look. A failure Kern cannot read stays a look. When the old
   test cannot even drive the new code (a mock lacks something the new code needs, a function it calls is gone), or Kern cannot run an
   edited test file's old version there (node's test runner, for one), the change is not verified. An edited data file, helper or
   mock that no old test names needs a look. A test file the change only adds tests to needs no such run:
   its old tests are still in it, unchanged, and ran on the new code. A new test that fails only for want of something the sandbox does
   not have (the network, a program or a native module) is listed as not checked.
2. **Whether the tests check the change at all.** When the tests pass, Kern undoes the change to each changed function of the product code,
   one function at a time (and the rest of a file's change on its own), and runs the tests related to that file (jest and vitest); then it
   plants up to three small faults spread over the changed lines (`>` turned into `>=`, a condition turned around, a call left out). Each
   is put in as the file is read, so nothing on disk is edited. Before them, Kern runs the same tests once with the file served unchanged,
   as a control: a test that fails there (a flaky one, or one that notices Kern's machinery) cannot catch anything. A changed function
   counts as checked when a test fails with its change undone, or when its change is a rewrite the tests guard: every small fault Kern put
   in it is caught, and no answer of it changed for the tests the change leaves untouched. The receipt says how many changed functions are
   checked, and its Tests line has no ✅ when some are not. When a test catches an undo, the receipt quotes what the test expects and what
   it got with that part of the change undone, so a reviewer judges the change as a few claims. For every changed function no test checks,
   the receipt gives the agent that wrote the change an exact task: which test file already runs it (with an input whose answer changed,
   when Kern saw one) and the assertion that is missing, or, for a rewrite meant to keep behaviour, to say so; where no test reaches the
   code, the agent may say in the pull request why it cannot be tested there instead of writing tests that mock a database. The agent
   pushes, and Kern checks again. Claims and tasks never change the answer. The receipt also names each changed file Kern put no fault in,
   with why (another test runner, no related tests it could pick, only comments, formatting, logging or TypeScript types changed, or its
   limit of 23 faults in 20 files per check). When no test that runs here even loads a changed file (its related tests cannot load, or
   there are none), that file is not verified, said plainly. A small fault that only puts back the old line, logging calls, and a run Kern
   could not finish are not counted, and when a related test file fails as a whole, a fault it alone might have loaded is not checked
   rather than missed. Then Kern watches what the changed code gives the tests this change does not touch: it maps the code (every product
   file, its exported functions and the imports between them, through barrels, path aliases and Vue script blocks), finds the files the
   change reaches, nearest first (up to 40), and rewrites the changed files and those neighbours so that their exported functions note
   every input and answer; it runs those tests (up to 40 files) on the old code twice and on the new code
   once, sets aside any input whose answer differs between the two old runs, and lists each input whose answer changed on the new code as a
   note, with both answers. These notes do not change the answer yet; Kern measures what they predict on real pull requests first. They
   come after the planted faults, or after one control run alone when there is no time to plant faults, and when time runs short for both they are dropped first.
3. **TypeScript errors.** For each TypeScript project the change touches, Kern runs the project's own `tsc` on the old and the new
   code, in the sandbox. It compares them only when the old code type-checks with no error at all there, so a missing generated file
   cannot make anything look new. New errors block when the project's own scripts run `tsc` (and the project has a lockfile, so both
   sides got the same dependencies), and need a look when its scripts do not run `tsc`. When they are about something a build step may
   generate, or the two sides got different dependencies, they are not verified. This works even when the tests cannot run.
4. **Whether the project still builds.** When the change touches code under a `package.json` whose `build` script exists on the old
   code too, Kern runs `npm run build` there on the old and the new code, in the sandbox, with the network off (the folder nearest the
   changed code, at most two). A build that worked on the old code and fails on the new code twice, the same way, needs a look that
   quotes its error; when the two sides got different dependencies, it is not verified. A build that already fails on the old code,
   needs the network or more memory, or fails only once is not compared, and the receipt says why. This works even when the tests
   cannot run.
5. **Test tricks.** An existing test file that newly skips, focuses or inverts tests (`.only`, `.skip`, `.skipIf`, `.fails`, `this.skip()`, under any
   name or chain), or code that reaches into the test runner,
   blocks. In a test file the change adds, it needs a look, since it cannot hide a test that passed before.
6. **New powers.** Changed code outside the tests that newly starts programs, reaches the network, runs `eval` or reads secret
   environment variables (a name whose last word is a credential, such as `API_KEY` or `DB_PASSWORD`; never one a framework builds into
   the page, such as `NEXT_PUBLIC_` or `VITE_`) needs a look. Tests run in the project's own CI, often next to its secrets, so a test
   needs a look when it newly starts programs or runs `eval`, or when it can both read a secret (or the whole environment) and reach the
   network, and one of those is new; a test that only calls `fetch`, or only sets `NODE_ENV`, does not. A power counts only when the code uses
   it: swapping `fetch` for a mock in a test, reading a class's name or checking that a value is a function is not one. Ordinary
   imports, such as a UI library, a project's own modules or a build tool's `virtual:` module, are not powers.
7. **Files that steer everything else.** Files that run code or grant access: CI and container files, `CODEOWNERS`, renovate's config,
   `.npmrc` and the like, and `.env` files that hold a secret or a setting that runs code (pictures, `FUNDING.yml`, notes such as
   Markdown files and `.env.example` templates, and `.env` files of plain settings aside). The settings of the test runner Kern runs
   (the test script in `package.json` included; a babel or vite config that runner never reads is not one), and the files that
   instruct coding agents (`AGENTS.md`, `CLAUDE.md`, `.cursorrules`) need a look too.
8. **Dependencies.** The receipt names what moved (`vitest 3.2.4 → 4.0.18`). An update within the same major version to a release
   that has been on npm for a week needs no look; a release less than a week old is named, because that is where hijacked packages
   show up. A package of this repository (`workspace:`, `file:` or a workspace's name) is the project's own, and a package moved to
   another section keeps its version. A new package, a major update, a `package.json` change that runs code on install or decides what
   gets installed (install scripts, `overrides`, `packageManager` and the like), and a lockfile entry that downloads a package from
   somewhere other than its own registry tarball need a look.
9. **Changes that break users without failing a test.** These need no test run, so they also cover projects whose tests Kern cannot
   run: tests removed from an existing test file, left checking nothing, or loosened to checks that pass for almost any value; a
   published package that stops exporting a name its users import; code that starts using a Node feature newer than the oldest Node
   the package says it supports (`engines.node`, checked against Node's own documentation); and HTML that adds inline styles or scripts
   that a Content-Security-Policy in the same change forbids.
10. **Requirements the change may break, found by AI and proven by running the code.** With an AI key, Kern asks Claude two questions
   in the job that holds the key. First, from what existed before the change (old tests, documentation and comments, never the new
   code), what must stay true about each changed exported function, each with a word-for-word quote of its source. Second, which
   realistic inputs could break those requirements. Kern checks each quote itself: it must be in a file that existed before the change
   and be about the function (inside a test that calls it, next to a line of the documentation that names it, or in its own comment).
   Then it calls the function with each input inside a copy of a test file that already loads it, through the project's own test
   runner (vitest or jest), on the old and the new code with the network off, and runs both again when an answer differs. Answers are
   compared as data: one that only adds fields, or prints differently but holds the same values, is a note, and so is a crash the old
   code made on an input it never handled. A requirement that the answer must not change is a look at most, because which inputs it
   covers is the AI's reading. Kern blocks only when a call that a requirement says must not throw now throws (or the reverse), the
   quote checks out and the change leaves its test case, snapshot or paragraph in place, inputs of that kind are ones existing tests or
   callers pass or the old code's plain parameter types allow, the real function answered, both old runs agree, both new runs agree,
   and both sides were set up the same way, build steps included. When the pull request says it changes that behaviour, adds or edits
   a test of that function, or a piece of that evidence is missing, it is a look that says which. Inputs that hang, reach for the
   network, answer differently within one run, or never reach the real function (a mock answered) are set aside and listed. The
   receipt gives the call, both answers and the quote, and a rerun of the same change reuses the AI's answers.
11. **Behaviour, outside the workflow for now.** `kern drive` has Claude Code, in a locked-down mode, write a small program that makes
   every changed line run; Kern runs it on the old and the new code and lists what behaves differently.

Where the code runs: Kern's workflow has two jobs. The job that installs and tests the pull request's code holds no keys at all (its
token can only read the repository) and runs on GitHub's throwaway machine, deleted when the job ends. There the code runs in locked
containers (read-only system, no privileges, an unprivileged user, memory and process limits, the network off while tests run), and
packages install with their own install scripts, the way CI installs them; when a script fails, Kern installs again without scripts.
A second job never runs the pull request's code: it builds Kern itself from source with no cache, renders the receipt from the first
job's results, signs it and posts it. With an AI key, a third job holds the key: it runs that same Kern build, checked by its hash,
reads the pull request's files and asks Claude, and never runs the pull request's code; the job that runs the code reads its answer
only as data. On a machine that is kept between jobs, the code runs in gVisor instead.

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
- The AI step can miss a regression, and it can cite a requirement Kern cannot confirm. Kern never blocks on the AI's word: a block
  needs the quote confirmed, a call that now throws or no longer throws against it, and the break repeated on both runs. A changed
  value is a look or a note, never a block, because no mechanical check says which inputs a "stays the same" requirement covers.
- Kern runs JavaScript and TypeScript today: the tests it runs are vitest, jest, mocha and `node --test`. When a change's tests cannot
  run (another runner, a failed install, no results, Kern's own environment failing), the answer is not verified and says why, never ✅;
  the rules that need no test run still apply, so a look they find is still a look. A
  project with no test script, whose own test script finds no test files, or whose test script only prints a message, gets a receipt that covers the rules only and says so;
  changed code in other languages is listed as "not checked", never passed quietly.
- Kern runs Node 18, 20, 22, 24 or 26, the major the project asks for, from the official images; a project that asks for another major
  runs on the next one Kern has. It runs a build step or `pretest` script only when the tests cannot load without it, or when the test
  script runs files from build output (`out/`, `dist/`) that are not in the repository; when the runner names a missing workspace package
  only after another is built, Kern builds it on a further pass, up to three passes. Tests that still cannot load cannot be checked,
  and the receipt says so with the first error instead of claiming anything. Temporary files the tests write go to a fresh folder on disk outside the project,
  as on the project's own CI, and a flood of test output is cut to its start and end, so neither fills the sandbox.
- Kern runs the tests with the network off. Tests that need the network, a database or a build step first fail before and after the
  change, so they are set aside, never blamed on it, and the receipt quotes why they did not run.
- A pull request from a fork gets its receipt in the job's summary only: GitHub gives such runs a read-only token and no signature, so
  Kern cannot comment or set its status there. The job fails unless the answer is ✅. If you require the `Kern` status, merge such a
  pull request after reading that summary.
- Kern reads the test runner's own report. Code running inside the tests shares the runner's process and could fake that report, as it
  could in any CI; Kern flags new test-runner settings and hooks, and the job that signs never runs that code. A test that notices Kern's
  planted faults and fails on purpose fails in the control run too, so it catches nothing; two records that disagree about one test's
  values are both ignored. A test written to hash its own source files can still tell a planted fault apart.
- A check has 55 minutes. Tests Kern had no time for are listed on the receipt as not run, and the answer is not verified, never passed or failed.
  Kern's own limits (time, memory, disk space, open files, a killed process, a broken report) never block.

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
