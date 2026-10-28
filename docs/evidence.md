# Evidence

What is measured on today's Kern, with the date it was measured. Newer results come first.

Back to the [overview](../README.md).

## 2026-10-28

| What | Result |
|---|---|
| 92 known tricks to hide new powers, through the check's own rules (`tests/redteam.rs`, `on_the_check_path_…`) | 84 need a look. 4 only write files, which the check does not count as a power. 4 reach the global object through a `this` that a plain non-strict function returns or keeps, which the check does not follow; that keeps jQuery's `$(this)` quiet. 28 of 28 realistic edits stay clear |

Results measured on designs Kern no longer ships (the twin, `kern drive`, contract version 5's VERIFIED and DENIED, `kern ingest`, the
hosted verifier and the merge gate) are in this file's git history before 2026-10-28. They say nothing about today's answers.
