# The shop: Kern's twin on Express and Postgres

The Phase 4 acceptance run. `sh run.sh` starts three containers and removes them all at the end:
- Postgres;
- the shop, with the twin serving through its own listener and recording Postgres;
- a traffic driver.

The driver sends 200 requests: lists, lookups, orders taken in a transaction and stamped with the clock, and order lookups. It passes when the twin compared at least 95% of them.

The new version has one real change: large orders show their total rounded. Those orders are reported as differences, while customers keep getting the old version's answers.
