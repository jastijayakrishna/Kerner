#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
docker build -q -t kern-twin-test:local -f scripts/test.Dockerfile scripts >/dev/null
if [ "$#" -eq 0 ]; then set -- 'test/**/*.test.js'; fi
exec docker run --rm -v "$PWD:/twin" -w /twin kern-twin-test:local node --test --test-concurrency=1 "$@"
