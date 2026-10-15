#!/bin/sh
# Runs the twin's sandbox tests (test/sandbox.test.js) in a Linux image with bubblewrap, as a user without root. bwrap needs a user
# namespace, which Docker's default seccomp and AppArmor profiles refuse inside a container, so this one container runs with both
# unconfined. Docker also masks parts of /proc, and the kernel then refuses the fresh /proc a ghost's sandbox mounts: systempaths=unconfined
# leaves /proc whole. It is removed at the end.
set -eu
cd "$(dirname "$0")/.."
docker build -q -t kern-twin-sandbox:local -f scripts/sandbox.Dockerfile scripts >/dev/null
docker run --rm --security-opt seccomp=unconfined --security-opt apparmor=unconfined --security-opt systempaths=unconfined -e KERN_TWIN_SANDBOX=1 \
  -v "$PWD:/twin" -w /twin kern-twin-sandbox:local node --test --test-concurrency=1 test/sandbox.test.js
