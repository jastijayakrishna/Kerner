#!/bin/sh
# The Phase 4 acceptance run: the Express and Postgres shop under the twin, driven by 200 requests. It passes when the twin compared at
# least 95% of them. Everything runs in throwaway containers of the compose project kern-twin-shop, removed at the end.
set -u
cd "$(dirname "$0")"
docker compose -p kern-twin-shop up --build --abort-on-container-exit --exit-code-from traffic
code=$?
docker compose -p kern-twin-shop down -v --rmi local
exit $code
