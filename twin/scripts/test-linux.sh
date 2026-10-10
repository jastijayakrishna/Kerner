#!/bin/sh
# Runs the twin's tests on Linux in the pinned Node 22 image (WSL's own Node is too old).
# Usage, from the repository root: sh twin/scripts/test-linux.sh [test files, relative to twin/]
set -eu
cd "$(dirname "$0")/.."
IMAGE=docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392
if [ "$#" -eq 0 ]; then set -- 'test/**/*.test.js'; fi
exec docker run --rm -v "$PWD:/twin" -w /twin "$IMAGE" node --test --test-concurrency=1 "$@"
