#!/bin/sh
# Kern's verifier end to end on real gVisor, from nothing. It installs gVisor when Docker lacks it (Ubuntu or Debian), sets up a
# throwaway verifier with Kern's own commands, and checks real changes to a small project:
#   an honest change is VERIFIED, and its signed proof passes the deploy gate while a tampered copy does not;
#   a change that breaks the tests, one that gives code new authority, and one that changes behaviour the task did not state
#   are refused.
# Usage: KERN_BIN=/path/to/static/linux/kern scripts/e2e-gvisor.sh     (CI runs it on every change)
set -eu
K=${KERN_BIN:?set KERN_BIN to a static Linux kern}
K=$(readlink -f "$K")
# The verifier copies itself into its sandboxes, whose images have their own C library: only the static build runs there.
if command -v file >/dev/null && ! file "$K" | grep -qE 'static(ally|-pie) linked'; then
  echo "e2e-gvisor: KERN_BIN must be the static Linux build (cargo build --release --target x86_64-unknown-linux-musl)" >&2; exit 1
fi
W=$(mktemp -d)
SUDO=; [ "$(id -u)" = 0 ] || SUDO=sudo
say() { printf '\n== %s\n' "$*"; }
fail() { printf '\ne2e-gvisor: FAILED: %s (everything is kept in %s)\n' "$*" "$W" >&2; exit 1; }
real() { readlink -f "$(command -v "$1")"; }
hash() { sha256sum "$1" | cut -d' ' -f1; }

say "gVisor registered with Docker as runsc"
if ! docker info --format '{{json .Runtimes}}' | grep -q '"runsc"'; then
  command -v apt-get >/dev/null || fail "Docker has no runsc runtime and this is not Ubuntu or Debian: install gVisor first"
  curl -fsSL https://gvisor.dev/archive.key | $SUDO gpg --dearmor --yes -o /usr/share/keyrings/gvisor-archive-keyring.gpg
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/gvisor-archive-keyring.gpg] https://storage.googleapis.com/gvisor/releases release main" \
    | $SUDO tee /etc/apt/sources.list.d/gvisor.list >/dev/null
  $SUDO apt-get update -qq && $SUDO apt-get install -y -qq runsc
  $SUDO runsc install
  $SUDO systemctl restart docker
fi
runsc=$(docker info --format '{{json .Runtimes.runsc}}' | jq -r .path)
runsc=$(readlink -f "$runsc")
"$runsc" --version | head -1

say "A throwaway verifier, set up with Kern's own commands"
cd "$W"
printf '{"host":"e2e","deployment":"single-vm","reviewed":"%s"}\n' "$(date -u +%F)" > host-policy.json
jq -n --arg docker "$(real docker)" --arg git "$(real git)" --arg runsc "$runsc" --arg version "$("$runsc" --version)" --arg policy "$W/host-policy.json" \
  --arg dh "$(hash "$(real docker)")" --arg gh "$(hash "$(real git)")" --arg rh "$(hash "$runsc")" --arg ph "$(hash host-policy.json)" \
  '{ version: 1, role: "verifier", docker: { file: $docker, sha256: $dh, endpoint: "unix:///var/run/docker.sock" }, git: { file: $git, sha256: $gh },
     runtime: { file: $runsc, sha256: $rh, version: $version, arguments: [] }, host_policy: { file: $policy, sha256: $ph } }' > verifier.json
"$K" --control verifier.json doctor > doctor.json || { cat doctor.json; fail "kern doctor says this machine cannot verify"; }
builder=$(jq -r .builder.sha256 doctor.json)
"$K" --control verifier.json keygen keys > /dev/null
docker pull -q node:22-bookworm-slim > /dev/null
image=$(docker image inspect --format '{{.Id}}' node:22-bookworm-slim)
mkdir checks
printf '{"version":1,"cases":[{"command":["/usr/local/bin/npm","test"],"stdout":null}]}\n' > checks/test-suite.json
jq -n --arg image "$image" --arg builder "$builder" --argjson check "$("$K" pin-check test-suite checks/test-suite.json)" \
  '{ version: 3, repository: "acme/demo", deployment: "single-vm", executors: ["external"], image: $image,
     builders: { executor: null, verifier: $builder }, checks: [$check], registry: null, permissions: {} }' > operator.json
task() { # name delta
  jq -n --arg name "$1" --argjson delta "$2" '{ version: 5, task: $name, filesystem: { read: ["src/", "test/"], write: ["src/", "test/"] },
    network: [], secrets: [], publish: false, dependencies: null,
    limits: { files_changed: 8, lines_changed: 400, file_bytes: 262144, total_bytes: 4194304, timeout_seconds: 300, memory_mb: 1024, pids: 256 },
    required_checks: ["test-suite"], behaviors: [], tests: null, delta: $delta, app: null }' > "task-$1.json"
}
task demo null
task tidy '{"intent":"Tidy the code; nothing changes","changes":[]}'

say "A small project and an agent's clone of it"
git init -q project && cd project
git config user.email e2e@example.invalid && git config user.name e2e
mkdir src test
printf '%s\n' '{"name":"demo","version":"1.0.0","private":true,"scripts":{"test":"node --test"}}' > package.json
printf '%s\n' 'exports.add = (a, b) => a + b' > src/add.js
printf '%s\n' "const test = require('node:test')" "const assert = require('node:assert')" "const { add } = require('../src/add')" \
  "test('adds', () => assert.equal(add(2, 3), 5))" > test/add.test.js
git add -A && git commit -qm base
base=$(git rev-parse HEAD)
cd "$W" && git clone -q project agent && git -C agent config user.email agent@example.invalid && git -C agent config user.name agent
for t in demo tidy; do
  jq -n --arg repo "$W/project" --arg base "$base" --arg c "$W/task-$t.json" --arg op "$W/operator.json" --arg key "$W/keys/public.pem" --arg out "$W/handoffs-$t" \
    '{ version: 3, repo: $repo, base: $base, contract: $c, operator: $op, command: null, verification_key: $key, output: $out }' > "job-$t.json"
done

change() { # name: the agent's commit for one scenario, from the base
  git -C agent checkout -q -B "$1" "$base"
  case "$1" in
    honest) printf '%s\n' 'exports.sub = (a, b) => a - b' >> agent/src/add.js
            printf '%s\n' "const test = require('node:test')" "const assert = require('node:assert')" "const { sub } = require('../src/add')" \
              "test('subtracts', () => assert.equal(sub(5, 3), 2))" > agent/test/sub.test.js ;;
    broken) printf '%s\n' 'exports.add = (a, b) => a + b + 1' > agent/src/add.js ;;
    authority) printf '%s\n' "const { execSync } = require('child_process')" 'exports.add = (a, b) => a + b' > agent/src/add.js ;;
    surprise) printf '%s\n' "exports.add = (a, b) => (typeof a === 'string' ? a.trim() + b : a + b)" > agent/src/add.js ;;
  esac
  git -C agent add -A && git -C agent commit -qm "$1" && git -C agent rev-parse HEAD
}
verify() { # name task: ingest, hand off, verify on gVisor; prints VERIFIED, DENIED: <reason> or BLOCKED: <reason>
  commit=$(change "$1")
  r=$("$K" ingest "job-$2.json" "$W/agent" "$commit" 2>&1) || true
  if [ "$(printf '%s' "$r" | jq -r .status 2>/dev/null)" != CANDIDATE ]; then printf 'BLOCKED: %s' "$(printf '%s' "$r" | jq -r '.reason // .' 2>/dev/null || printf '%s' "$r")"; return 0; fi
  "$K" --control verifier.json import "$(printf '%s' "$r" | jq -r .bundle)/handoff" "run-$1" > /dev/null
  if "$K" --control verifier.json verify "run-$1" "task-$2.json" operator.json "out-$1" keys/private.pem > "out-$1.json" 2> "out-$1.err"; then echo VERIFIED
  else printf 'DENIED: %s' "$(jq -r '.reason // .' "out-$1.err" 2>/dev/null || cat "out-$1.err")"; fi
}
expect() { # name task wanted-start-of-verdict
  started=$(date +%s)
  got=$(verify "$1" "$2")
  printf '%-10s %s (%ss)\n' "$1" "$(printf '%s' "$got" | cut -c1-220)" "$(( $(date +%s) - started ))"
  case "$got" in "$3"*) ;; *) fail "$1: expected a verdict starting with \"$3\"" ;; esac
}

say "Changes verified on gVisor"
expect honest demo "VERIFIED"
expect broken demo "DENIED: Check test-suite case 1 failed"
expect authority demo "DENIED: Patch widens module capabilities"
expect surprise tidy "DENIED: Behaviour changed outside the task's stated change"

say "The signed proof at the deploy gate"
mkdir -p trusted/tasks && cp operator.json trusted/ && cp keys/public.pem trusted/verifier.pub && cp task-demo.json trusted/tasks/demo.json
# The proof covers the commit Kern built at hand-off (the agent's tree on the base), which is what a promotion publishes and a deploy ships.
signed=$(jq -r .commit run-honest/run.json)
"$K" gate-artifact "$W/run-honest/objects.git" "$signed" out-honest/attestation.json "$W/trusted" > gate.json || { cat gate.json; fail "the deploy gate refused a VERIFIED change"; }
echo "the verified commit: $(jq -r .status gate.json)"
if "$K" gate-artifact "$W/run-honest/objects.git" "$base" out-honest/attestation.json "$W/trusted" > /dev/null 2>&1; then fail "the deploy gate accepted the proof for another commit"; fi
echo "the same proof for another commit: refused"
jq '.payload |= (.[0:-8] + "AAAAAAA=")' out-honest/attestation.json > tampered.json
if "$K" gate-artifact "$W/run-honest/objects.git" "$signed" tampered.json "$W/trusted" > /dev/null 2>&1; then fail "the deploy gate accepted a tampered proof"; fi
echo "a tampered proof: refused"

say "All verifier checks passed"
cd / && rm -rf "$W"
