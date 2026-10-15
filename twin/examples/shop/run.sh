#!/bin/sh
set -u
cd "$(dirname "$0")"
docker compose -p kern-twin-shop up --build --abort-on-container-exit --exit-code-from traffic
code=$?
docker compose -p kern-twin-shop down -v --rmi local
exit $code
