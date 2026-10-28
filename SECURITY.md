# Security model

Kern posts one signed receipt on a pull request. It does not make agents, patches or hosts safe in general, and a receipt is not proof that code is correct or well meant: a person still decides whether a change is good.

## Trust

Everything that comes from the pull request is untrusted: its code, tests, package scripts, lockfiles, file names, and its title and body. The title and body reach the AI step, so they can carry prompt injection.

- **The run job.** The pull request's code runs only here. The job has no keys and a read-only token. The code runs in locked containers (read-only system, no privileges, an unprivileged user, memory and process limits) with the network off while tests run. The machine is thrown away afterwards; on a kept machine, gVisor is used instead.
- **The build job.** It builds Kern from source with no cache, on a machine where no pull request code runs, and hands the binary on with its hash.
- **The sign job.** It never runs or loads anything the run job could have touched, not even a cache. It runs the build job's Kern, checked by its hash, treats the run job's results as data, rebuilds the answer from the receipt's facts and signs it with GitHub's OIDC key.
- **The investigate job.** The only job that holds the AI key. It runs the Kern binary the build job made, checked by its hash, reads the pull request's files through git and never runs them. The model's answers are untrusted input: Kern checks every quote and input mechanically, and the run job reads the investigation only as data.
- **Customers** add one workflow file. There is no Kern server; checks run on the repository's own GitHub Actions minutes.

## Guarantees

- **The receipt.** Its answer (`clear`, `unverified`, `look` or `blocked`) is rebuilt from the recorded facts in the sign job, so the run job cannot write an answer the facts do not support. GitHub signs the whole receipt; an edited comment fails `kern receipt verify`, and `/kern approve` counts only signed receipts.
- **A block needs two matching failures.** A test must fail in every new run that included it and pass in every old run, including an old rerun, with both sides set up the same way and the failure not one the sandbox causes (network, git, a service).
- **Dependency setup has network access, nothing else does.** Packages install with their own install scripts, as CI installs them, in a container that holds no keys; the tests, the type check, the build steps and the planted faults run with the network off.
- **What Kern cannot check is `unverified`, never green.** A sandbox limit, a report Kern cannot read, a changed file that steers what runs or what gets installed, or a change no test reaches is said plainly.

## Known limits

- **The capability check is syntactic.** It finds new powers (programs started, the network reached, secrets read); misuse of authority a module already holds needs tests and review. Identifiers are resolved with `oxc_semantic`; `tests/redteam.rs` tracks known evasions.
- **Tests that fail before the change are set aside.** They can neither block nor prove anything, so a change can leave them broken; the receipt lists them.
- **The safety net for rewritten tests is a sample.** A small set of planted faults decides whether rewritten or removed tests still catch what the old ones caught.
- **Kern trusts the test runner's report.** Code running inside the test process can tamper with what the runner reports; Kern's own reporters record what they see, not what the process is. Changed code that names Kern's own machinery (`KERN_` variables, `/kern/`, `.kern-` files) needs a look, but that is a tripwire: a test can still tell a planted fault apart without naming any of it, for example by comparing a function's text with its file.
- **Install scripts run as they do in CI.** They run as root in the install container, with the network on, so they can change what the tests later load, Kern's own Bun and corepack included once Kern has installed them there. Kern makes its tool folders afresh, never reuses a tool it did not install in the same setup, and makes its launchers after the installs, so a committed file or an earlier script cannot plant one. A changed install script, a changed file an install script runs, and a changed patch an install applies need a look; a new dependency's own install script does not.
- **Kern's status and Kern's build share the repository with its other workflows.** Any workflow in a repository can set a commit status; the receipt's signature, not the status, is what `kern receipt verify` checks.
- **The installers trust the release they come from.** `install.sh` and `install.ps1` check the binary against that release's `SHA256SUMS`, which catches a corrupted or swapped download, not a release built from the wrong source.

## Reporting

Report vulnerabilities privately to the repository owner. Never include credentials.
