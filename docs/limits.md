# What Kern does not do yet

Kern checks a pull request with the project's own tests and its own rules. It does not prove that code is correct or well meant: a person still decides whether a change is good.

Back to the [overview](../README.md).

## The check

- The capability check is static: it names new authority a change grants (a look), not misuse of authority a module already has. Names are resolved through `oxc_semantic`, so `process` or `self` count only when they are the real globals. A `this` that a plain non-strict function returns or keeps as a value (`return this`, `var self = this`) is not followed, so code that reaches `fetch` or `process` through it is not seen: 4 of the 92 tricks in `tests/redteam.rs`. That keeps jQuery's `$(this)` quiet. Writing files is not a power the check looks for: 4 more.
- Planted faults come from a small set of edits, so the safety net for rewritten tests is a strong sample, not a proof.
- Only the last package Kern tests in a check can have a run longer than 15 minutes: its new code's run, its whole reruns and its related tests. A package that another package's tests follow keeps 15 minutes per run on each side, so the next one still gets its time; when its new code needs more, that package is not verified. The related tests' limit counts the whole suite's full 15 minutes even when the early stop ended it sooner, so a second check of the same pull request gets the same limit, give or take its setup time; the time an early stop saves goes to the work after the related tests. A run that needs about as long as its limit can still finish in one check and not in the next. A run cut short gives no results at all: Kern does not yet keep the test files that finished before the cut. The five-minute reserve leaves room for single-file reruns, not for whole reruns of a long related set: when those do not fit, the failure is not verified, never blocked.
- A pinned action moved from one commit to another needs a look even when the new commit is the release its comment names: Kern does not look the tag up yet.
- A test can tell a planted fault apart from the real code without naming Kern's machinery, for example by comparing a function's text with its file on disk. Changed code that names it (the `KERN_` variables Kern sets in the sandbox, `/kern/` paths, `.kern-` files) needs a look; that is a tripwire, not a proof. A name built at run time (`'KE' + 'RN_'`) is not seen.
- On a throwaway machine (runc mode, as on GitHub's hosted runners), install scripts run as they do in CI: as root in the install container, with the network on. On a kept machine (gVisor) they do not run. They can change what the tests load, Kern's own Bun and corepack included once Kern has installed them there. A changed install script, a changed file an install script runs and a changed patch need a look; a new dependency's own install script does not.
- The temporary folder the tests write to has no size limit. What Kern answers when a test fills the machine's disk is not tested yet.

See [SECURITY.md](../SECURITY.md) for the trust model.
