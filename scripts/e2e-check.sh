#!/bin/sh
# kern check end to end on real gVisor: a small project with vitest tests and six changes, each with the receipt it must get.
#   harmless   a refactor; the tests still pass                      -> clear
#   breaks     the discount starts one euro later; a test now fails  -> blocked (a test that passed before now fails)
#   only       a test file focuses one test with .only               -> blocked (a trick)
#   power      the code starts a program                             -> needs a look (a new power)
#   docs       only the README changes                               -> clear, and nothing is installed or run
#   writes     a test writes into the project, before and after      -> clear: a test that failed before is set aside, never blamed
#   mono       a monorepo whose root runs no tests; a package's test breaks -> blocked: the package's own tests ran, from its folder
# The AI part is off (KERN_CHECK_AI=0); scripts/e2e-drive.sh covers it. Needs Docker with runsc, jq and git; installs from npm.
# Usage: KERN_BIN=/path/to/static/linux/kern scripts/e2e-check.sh
set -eu
K=${KERN_BIN:?set KERN_BIN to a static Linux kern}
K=$(readlink -f "$K")
W=$(mktemp -d)
export KERN_CHECK_AI=0
say() { printf '\n== %s\n' "$*"; }
fail() { printf '\ne2e-check: FAILED: %s (everything is kept in %s)\n' "$*" "$W" >&2; exit 1; }
real() { readlink -f "$(command -v "$1")"; }
hash() { sha256sum "$1" | cut -d' ' -f1; }
volumes() { docker volume ls --quiet | grep -c '^kern-deps-' || true; }
before=$(volumes)

say "A throwaway verifier"
cd "$W"
runsc=$(readlink -f "$(docker info --format '{{json .Runtimes.runsc}}' | jq -r .path)")
printf '{"host":"e2e-check","deployment":"single-vm","reviewed":"%s"}\n' "$(date -u +%F)" > host-policy.json
jq -n --arg docker "$(real docker)" --arg git "$(real git)" --arg runsc "$runsc" --arg version "$("$runsc" --version)" --arg policy "$W/host-policy.json" \
  --arg dh "$(hash "$(real docker)")" --arg gh "$(hash "$(real git)")" --arg rh "$(hash "$runsc")" --arg ph "$(hash host-policy.json)" \
  '{ version: 1, role: "verifier", docker: { file: $docker, sha256: $dh, endpoint: "unix:///var/run/docker.sock" }, git: { file: $git, sha256: $gh },
     runtime: { file: $runsc, sha256: $rh, version: $version, arguments: [] }, host_policy: { file: $policy, sha256: $ph } }' > verifier.json
"$K" --control verifier.json doctor > doctor.json || { cat doctor.json; fail "kern doctor says this machine cannot run the sandbox"; }
docker pull -q node:22-bookworm-slim > /dev/null
image=$(docker image inspect --format '{{.Id}}' node:22-bookworm-slim)
mkdir checks && printf '{"version":1,"cases":[{"command":["/usr/local/bin/npm","test"],"stdout":null}]}\n' > checks/test-suite.json
jq -n --arg image "$image" --arg builder "$(jq -r .builder.sha256 doctor.json)" --argjson check "$("$K" pin-check test-suite checks/test-suite.json)" \
  '{ version: 3, repository: "acme/calc", deployment: "single-vm", executors: ["external"], image: $image,
     builders: { executor: null, verifier: $builder }, checks: [$check], registry: null, permissions: {} }' > operator.json

say "A project with tests"
git init -q calc && cd calc
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"calc","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
cat > src/total.js <<'EOF'
// The price of a basket: each item's price times its quantity.
export function total(items) {
  return items.reduce((sum, item) => sum + item.price * item.qty, 0);
}

// Baskets of 100 or more get 10% off.
export function discount(amount) {
  return amount >= 100 ? amount * 0.9 : amount;
}
EOF
cat > test/total.test.js <<'EOF'
import { test, expect } from 'vitest';
import { total, discount } from '../src/total.js';

test('total adds price times quantity', () => expect(total([{ price: 2, qty: 3 }, { price: 1, qty: 1 }])).toBe(7));
test('discount from 100', () => expect(discount(100)).toBe(90));
test('no discount below 100', () => expect(discount(99)).toBe(99));
EOF
printf '# calc\n' > README.md
git add -A && git commit -qm base && base=$(git rev-parse HEAD)

# Each change is a branch from the base; `change NAME MESSAGE` commits what the lines before it did.
change() { git add -A && git commit -qm "$2" && git rev-parse HEAD > "$W/$1.head" && git checkout -q "$base"; }
git checkout -q -b harmless "$base"
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' src/total.js
change harmless "Shorter names"
git checkout -q -b breaks "$base"
sed -i 's/amount >= 100/amount > 100/' src/total.js
change breaks "Discount above 100"
git checkout -q -b only "$base"
sed -i "s/^test('discount from 100'/test.only('discount from 100'/" test/total.test.js
change only "Focus on the discount"
git checkout -q -b power "$base"
printf '%s\n' "import { execSync } from 'node:child_process';" '' "export const version = () => execSync('git describe --tags').toString().trim();" >> src/total.js
change power "Show the version"
git checkout -q -b docs "$base"
printf '# calc\n\nAdds up baskets.\n' > README.md
change docs "Say what calc does"
git checkout -q -b writes "$base"
cat > test/files.test.js <<'EOF'
import { test, expect } from 'vitest';
import { writeFileSync } from 'node:fs';

test('a test that writes into the project', () => { writeFileSync(new URL('./out.txt', import.meta.url), 'x'); expect(1).toBe(1); });
EOF
git add -A && git commit -qm "A test that writes a file" && writes_base=$(git rev-parse HEAD)
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' src/total.js
change writes "Shorter names, with a test that writes"
cd "$W"

# check NAME BASE [REPO]: runs kern check on the change (in calc, or REPO) and keeps its summary in NAME.json.
check() {
  printf '# %s\n' "$1" > "$1.md"
  started=$(date +%s)
  "$K" --control verifier.json check "$W/${3:-calc}" "$2" "$(cat "$1.head")" "$W/$1" operator.json "$1.md" > "$1.json" 2> "$1.err" || { cat "$1.err"; fail "$1: kern check failed"; }
  echo "$1: $(jq -c '{decision, blocked, look, tests, behaviour}' "$1.json") in $(( $(date +%s) - started )) s"
}
decision() { [ "$(jq -r .decision "$1.json")" = "$2" ] || { cat "$1/receipt.md"; fail "$1: expected $2, got $(jq -r .decision "$1.json")"; }; }
kinds() { jq -r --arg k "$2" '[.'"$3"'[] | select(.kind == $k)] | length' "$1/receipt.json"; }

say "A harmless change is clear, and every test still passes"
check harmless "$base"
decision harmless clear
[ "$(jq -r .tests.status harmless/receipt.json)" = RAN ] || { jq .tests harmless/receipt.json; fail "harmless: the tests did not run"; }
[ "$(jq -r .tests.kept harmless/receipt.json)" = 3 ] || { jq .tests harmless/receipt.json; fail "harmless: expected 3 tests kept"; }
grep -q '3 tests passed before and still pass' harmless/receipt.md || { cat harmless/receipt.md; fail "harmless: the receipt does not say the tests still pass"; }
grep -q '^### ✅ Kern: Nothing unexpected$' harmless/receipt.md || { cat harmless/receipt.md; fail "harmless: wrong headline"; }

say "A change that breaks a test is blocked, and the receipt names the test"
check breaks "$base"
decision breaks blocked
[ "$(kinds breaks test-broke blocked)" = 1 ] || { jq .tests breaks/receipt.json; fail "breaks: expected one broken test"; }
grep -q 'discount from 100' breaks/receipt.md || { cat breaks/receipt.md; fail "breaks: the broken test is not named"; }

say "A focused test is blocked as a trick, and the tests it hides are named"
check only "$base"
decision only blocked
[ "$(kinds only trick blocked)" -ge 1 ] || { jq .rules only/receipt.json; fail "only: .only was not reported as a trick"; }
cat only/receipt.md

say "A new power needs a look"
check power "$base"
decision power look
[ "$(kinds power power look)" = 1 ] || { jq .rules.powers power/receipt.json; fail "power: starting a program was not reported"; }
grep -q 'src/total.js' power/receipt.md || fail "power: the file is not named"

say "A docs-only change is clear, and Kern installs and runs nothing"
check docs "$base"
decision docs clear
[ "$(jq -r .tests.status docs/receipt.json)" = NOTHING_TO_RUN ] || fail "docs: the tests ran for a README change"
[ ! -f docs/setup.json ] || fail "docs: Kern installed the project for a README change"
grep -q '^### ✅ Kern: Nothing unexpected$' docs/receipt.md && grep -q 'nothing to run' docs/receipt.md || { cat docs/receipt.md; fail "docs: the receipt must say there was nothing to run"; }

say "A test that fails before and after is set aside, never blamed on the change"
check writes "$writes_base"
decision writes clear
jq -e '.tests.already_failing | length == 1' writes/receipt.json > /dev/null || { jq .tests writes/receipt.json; fail "writes: the test that cannot write was not set aside"; }

say "In a monorepo whose root runs no tests, the tests of the package the change touches run"
git init -q monorepo && cd monorepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p packages/calc/src packages/calc/test
printf '%s\n' '{"name":"mono","private":true,"workspaces":["packages/*"]}' > package.json
printf '%s\n' '{"name":"@mono/calc","version":"1.0.0","private":true,"type":"module","devDependencies":{"vitest":"3.2.4"}}' > packages/calc/package.json
git -C "$W/calc" show "$base:src/total.js" > packages/calc/src/total.js
git -C "$W/calc" show "$base:test/total.test.js" > packages/calc/test/total.test.js
git add -A && git commit -qm base && mono_base=$(git rev-parse HEAD)
sed -i 's/amount >= 100/amount > 100/' packages/calc/src/total.js
git commit -qam "Discount above 100" && git rev-parse HEAD > "$W/mono.head"
cd "$W"
check mono "$mono_base" monorepo
decision mono blocked
[ "$(kinds mono test-broke blocked)" = 1 ] || { jq .tests mono/receipt.json; fail "mono: expected one broken test in the package"; }
grep -q 'packages/calc/test/total.test.js' mono/receipt.md && grep -q 'vitest in packages/calc' mono/receipt.md || { cat mono/receipt.md; fail "mono: the package's test and runner are not named"; }

say "The sandbox volumes are removed"
for c in harmless breaks only power writes mono; do "$K" --control verifier.json drive clean "$W/$c" > /dev/null; done
after=$(volumes)
[ "$after" -le $(( before + 1 )) ] || fail "kern check left $(( after - before )) sandbox volumes behind"

printf '\ne2e-check: PASSED\n'
rm -rf "$W"
