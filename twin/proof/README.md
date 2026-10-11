# The twin's proof on real apps

Each app is run three times, each time in throwaway containers on a fresh database:
1. without the twin, as the baseline;
2. with the twin copying 10% of requests;
3. with the twin copying all of them.

The same seeded traffic is sent each time. The results land in `reports/twin-proof-<date>/`. Then:

```bash
node twin/proof/kit/report.mjs reports/twin-proof-<date>     # writes reports/twin-proof-<date>.md
```

- `kit/`: the numbers (`stats.mjs`), the traffic driver (`driver.mjs`), the app's server (`server.mjs`), the runner (`traffic.mjs`) and the report (`report.mjs`).
- `run.sh <app> <minimum free MB>`: one app's three runs. It refuses to start below that much free memory.
- One folder per app:
  - `compose.yml`;
  - an image;
  - `config.json` for the server;
  - `scenario.mjs` for the traffic;
  - the glue the app needed. That glue is counted in the report as the cost of adopting the twin.

Run it in WSL, one app at a time: `sh twin/proof/run.sh shop 1500`.
