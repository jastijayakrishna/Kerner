#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
PG=docker.io/library/postgres:17-alpine@sha256:742f40ea20b9ff2ff31db5458d127452988a2164df9e17441e191f3b72252193
REDIS=docker.io/library/redis:7-alpine@sha256:6ab0b6e7381779332f97b8ca76193e45b0756f38d4c0dcda72dbb3c32061ab99
run="kern-twin-stacks-$$"
cleanup() {
  docker rm -f "$run-pg" "$run-redis" >/dev/null 2>&1 || true
  docker network rm "$run" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker build -q -t kern-twin-stacks:local test/stacks >/dev/null
docker network create "$run" >/dev/null
docker run -d --rm --name "$run-pg" --network "$run" -e POSTGRES_PASSWORD=kern "$PG" >/dev/null
docker run -d --rm --name "$run-redis" --network "$run" "$REDIS" >/dev/null
i=0
until docker exec "$run-pg" pg_isready -U postgres >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -gt 60 ]; then echo "Postgres did not start"; exit 1; fi
  sleep 1
done
docker run --rm --network "$run" -v "$PWD:/stacks/twin" -w /stacks/twin \
  -e KERN_TWIN_PG_URL="postgres://postgres:kern@$run-pg:5432/postgres" -e KERN_TWIN_REDIS_URL="redis://$run-redis:6379" \
  kern-twin-stacks:local node --test --test-concurrency=1 --test-force-exit test/stacks/stacks.test.js
