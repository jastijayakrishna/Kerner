# What Kern does not do yet

Kern narrows what an AI agent's change can do and shows you what it really does. It does not prove that code is correct or well meant: a person still decides whether a change is good.

Back to the [overview](../README.md).

## The check

- The capability check is static: it stops new authority from being granted, not misuse of authority a module already has. Names are resolved through `oxc_semantic`, so `process` or `self` count only when they are the real globals, and `this` counts as the global object inside plain non-strict functions. New code that passes such a `this` on as a value (for example `$(this)` or `fn.apply(this, arguments)` in a non-strict file) is refused; existing code is covered by its baseline.
- Planted faults come from a small set of edits, so the safety net for rewritten tests is a strong sample, not a proof.
- Only the last package Kern tests in a check can have a run longer than 15 minutes: its new code's run, its whole reruns and its related tests. A package that another package's tests follow keeps 15 minutes per run on each side, so the next one still gets its time; when its new code needs more, that package is not verified. The related tests' limit counts the whole suite's full 15 minutes even when the early stop ended it sooner, so a second check of the same pull request gets the same limit, give or take its setup time; the time an early stop saves goes to the work after the related tests. A run that needs about as long as its limit can still finish in one check and not in the next. A run cut short gives no results at all: Kern does not yet keep the test files that finished before the cut. The five-minute reserve leaves room for single-file reruns, not for whole reruns of a long related set: when those do not fit, the failure is not verified, never blocked.

## The twin

The twin's own list is in [twin/README.md](../twin/README.md#not-yet). The proof on real apps also found these, still open:

- A request the old version refused (a 4xx answer) that the new version now answers by writing something is labelled `effects`, not `door-opened`, so it does not yet require a person's decision.
- `kern-twin test` cannot yet replay saved cases of an app that needs `load: 'record'` (one that reads its database or cache while it starts).
- An app whose requests depend on shared state built up by earlier requests (a rate limiter's counter, a cache filled in the background) is often not compared: the twin declines rather than guesses, so its evidence gathers more slowly.

See [SECURITY.md](../SECURITY.md) for the trust model.
