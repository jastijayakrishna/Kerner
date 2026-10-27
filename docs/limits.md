# What Kern does not do yet

Kern narrows what an AI agent's change can do and shows you what it really does. It does not prove that code is correct or well meant: a person still decides whether a change is good.

Back to the [overview](../README.md).

## The check

- The capability check is static: it stops new authority from being granted, not misuse of authority a module already has. Names are resolved through `oxc_semantic`, so `process` or `self` count only when they are the real globals, and `this` counts as the global object inside plain non-strict functions. New code that passes such a `this` on as a value (for example `$(this)` or `fn.apply(this, arguments)` in a non-strict file) is refused; existing code is covered by its baseline.
- Planted faults come from a small set of edits, so the safety net for rewritten tests is a strong sample, not a proof.

## The twin

The twin's own list is in [twin/README.md](../twin/README.md#not-yet). The proof on real apps also found these, still open:

- A request the old version refused (a 4xx answer) that the new version now answers by writing something is labelled `effects`, not `door-opened`, so it does not yet require a person's decision.
- `kern-twin test` cannot yet replay saved cases of an app that needs `load: 'record'` (one that reads its database or cache while it starts).
- An app whose requests depend on shared state built up by earlier requests (a rate limiter's counter, a cache filled in the background) is often not compared: the twin declines rather than guesses, so its evidence gathers more slowly.

See [SECURITY.md](../SECURITY.md) for the trust model.
