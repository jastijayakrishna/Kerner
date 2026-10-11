#!/bin/sh
# One app's proof: three runs in throwaway containers (without the twin, 10% copied, all copied), each on a fresh database. Results go
# to reports/twin-proof-<date>/. Usage, in WSL: sh twin/proof/run.sh <app> <minimum free MB>
set -u
app="$1"
need="${2:-1500}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="$root/reports/twin-proof-$(date +%F)"
mkdir -p "$out"
free_mb() { free -m | awk '/^Mem:/ {print $7}'; }
if [ "$(free_mb)" -lt "$need" ]; then
  echo "only $(free_mb) MB free and $need MB needed: drop the WSL cache, or ask the user to free memory" >&2
  exit 2
fi
compose="docker compose -p kern-twin-proof-$app -f $here/$app/compose.yml"
code=0
for mode in off 0.1 1; do
  if [ "$mode" = off ]; then
    export KERN_PROOF_MODE=off KERN_PROOF_SAMPLE=0
  else
    export KERN_PROOF_MODE=twin KERN_PROOF_SAMPLE="$mode"
  fi
  export KERN_PROOF_OUT="$out"
  $compose up --build --abort-on-container-exit --exit-code-from traffic || code=$?
  $compose down -v
done
$compose down -v --rmi local
exit $code
