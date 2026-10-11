#!/bin/sh
# A first start: one app under the twin, every request copied. It builds and starts the app, waits until it listens, sends one request
# through the twin, prints the twin's numbers, and removes the containers. Usage, in WSL: sh twin/proof/start.sh <app> <path>
set -u
app="$1"
path="${2:-/}"
here="$(cd "$(dirname "$0")" && pwd)"
compose="docker compose -p kern-twin-proof-$app -f $here/$app/compose.yml"
export KERN_PROOF_MODE=twin KERN_PROOF_SAMPLE=1 KERN_PROOF_OUT="${TMPDIR:-/tmp}/kern-proof-start"
$compose up -d --build app
for i in $(seq 1 120); do
  if $compose logs app 2>/dev/null | grep -q 'listening on'; then break; fi
  # An app container that stopped will never listen: show its log at once.
  if ! $compose ps --status running --services 2>/dev/null | grep -qx app; then break; fi
  sleep 5
done
$compose logs app | tail -20
$compose exec -T app node /twin/proof/kit/check.mjs "$path"
code=$?
$compose down -v
exit $code
