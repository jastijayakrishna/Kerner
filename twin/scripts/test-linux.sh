#!/bin/sh
# Runs the twin's tests on Linux in the pinned Node 22 image with git added (WSL's own Node is too old). The image is built once and
# kept; the test container is removed when it ends.
# Usage, from the repository root: sh twin/scripts/test-linux.sh [test files, relative to twin/]
set -eu
cd "$(dirname "$0")/.."
docker build -q -t kern-twin-test:local -f scripts/test.Dockerfile scripts >/dev/null
if [ "$#" -eq 0 ]; then set -- 'test/**/*.test.js'; fi
exec docker run --rm -v "$PWD:/twin" -w /twin kern-twin-test:local node --test --test-concurrency=1 "$@"
