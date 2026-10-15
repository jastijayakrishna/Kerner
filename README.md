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

## How Kern helps: three checks at three moments

| When | Part of Kern | What it does | What you get |
|---|---|---|---|
| Before merge | **Merge gate** | Checks the change in a sealed sandbox: what it touches, any new powers, the real tests, and the old code's answers against the new code's | A signed proof. GitHub blocks the merge without it |
| On the pull request | **`kern drive`** | Makes every changed line run, on the old and on the new code | One comment: what behaves differently, and which lines never ran |
| In production | **Twin** | The new version answers copies of real requests in secret, beside the old version | The new version serves customers only after it has earned it, and is switched back at the first surprise |

Each part works on its own.

### 1. Merge gate: nothing merges without proof

```text
agent's change ──▶ Kern checks it in a sealed sandbox ──▶ signed proof ──▶ GitHub check "kern-verified" ──▶ merge
                          │
                          └── blocked? The agent gets plain instructions and tries again.
```

Kern checks five things:

1. **Scope.** The change stays inside the folders the task allows, and under a size limit. CI settings and secret files are always off limits. Dependency files may change only for the packages you approve.
2. **No new powers.** Code may not newly start programs, reach the network, read environment variables or run `eval`, unless you allow it.
3. **Honest tests.** The project's own tests run on the exact commit, with no network. Every test that passed before must still pass. A bug fix needs a new test that fails before the fix and passes after it. Deleted, skipped and faked tests are caught.
4. **No surprises.** Kern calls the changed functions with the same inputs on the old and the new code, and compares every answer. A new crash, a removed check, a deleted file, a page that now lets the wrong user in, slower code or more paid AI calls is blocked or shown to you.
5. **Proof.** A separate verifier signs the result. GitHub checks the signature before merge. An audit report for any period re-checks every merged change, and maps to the SOC 2 CC8.1 and ISO 27001 change-control requirements.

Read more: [docs/merge-gate.md](docs/merge-gate.md)

### 2. `kern drive`: see what a change does

Tests only cover what someone tested. With `kern drive`, an AI agent writes a small program that makes every changed line run. By default the agent is Claude Code in a locked-down mode: it can only read and write its own task folder, and it cannot run commands or open web pages. Kern runs the program on the old and the new code, twice each, in a sealed sandbox with the network off. Then it writes one comment on the pull request:

- what behaves differently, with the changes the pull request does not mention listed first;
- which changed lines never ran, so a reviewer knows exactly what is left to read.

It takes about 6 minutes, and about $0.27 of AI use, per pull request.

Read more: [docs/drive.md](docs/drive.md)

### 3. Twin: let real traffic prove the change

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
- Linux with Docker and gVisor, for the sealed sandbox (on Windows, use WSL2).

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
- Kern runs JavaScript and TypeScript today. Changed code in other languages is listed as "not checked", never passed quietly.

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
