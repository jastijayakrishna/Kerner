#!/bin/sh
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
printf '%s\n' "import { defineConfig } from 'vitest/config';" 'export default defineConfig({ test: {} });' > vitest.config.mts
git add -A && git commit -qm "A test that writes a file, and a TypeScript config vite bundles next to itself" && writes_base=$(git rev-parse HEAD)
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' src/total.js
change writes "Shorter names, with a test that writes"
cd "$W"

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
grep -q '3 that passed before still pass' harmless/receipt.md || { cat harmless/receipt.md; fail "harmless: the receipt does not say the tests still pass"; }
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

say "Tests run on their own side's tree, so a test that writes into the project, and vite's bundled TypeScript config, work"
check writes "$writes_base"
decision writes clear
jq -e '.tests.status == "RAN" and .tests.kept == 4 and (.tests.already_failing | length == 0)' writes/receipt.json > /dev/null || { jq .tests writes/receipt.json; fail "writes: the tests that write did not run as they do in CI"; }

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

say "Tests cannot change committed code while they run, and code a test writes into the project needs a look"
git -C calc checkout -q -b plants "$base"
cat > calc/test/plant.test.js <<'EOF'
import { test, expect } from 'vitest';
import { writeFileSync } from 'node:fs';

test('plants a module', () => { writeFileSync(new URL('../src/shadow.mjs', import.meta.url), 'export const x = 1;\n'); expect(1).toBe(1); });
test('cannot rewrite committed code', () => { expect(() => writeFileSync(new URL('../src/total.js', import.meta.url), 'export const total = () => 0;\n')).toThrow(); });
EOF
git -C calc add -A && git -C calc commit -qm "Tests that write into the project" && git -C calc rev-parse HEAD > plants.head && git -C calc checkout -q "$base"
check plants "$base"
decision plants look
[ "$(kinds plants wrote-code look)" = 1 ] && grep -q 'src/shadow.mjs' plants/receipt.md || { cat plants/receipt.md; fail "plants: the module a test wrote was not reported"; }
jq -e '.tests.kept == 3 and (.tests.new_failing | length == 0)' plants/receipt.json > /dev/null || { jq .tests plants/receipt.json; fail "plants: committed code could be rewritten during a test run, or the tests did not run"; }

say "A change after which the tests give no results, where they gave results before, is blocked"
git -C calc checkout -q -b broke "$base"
printf '%s\n' "throw new Error('the test config cannot load');" > calc/vitest.config.mjs
git -C calc add -A && git -C calc commit -qm "A test config that throws" && git -C calc rev-parse HEAD > broke.head && git -C calc checkout -q "$base"
check broke "$base"
decision broke blocked
[ "$(kinds broke test-run-broke blocked)" = 1 ] || { jq .tests broke/receipt.json; fail "broke: tests that give nothing on the new code were not blocked"; }

say "A check has one time limit, and a run it cuts short is reported as not run, never as a failure"
git -C calc checkout -q -b late "$base"
printf '%s\n' "import { test } from 'vitest';" '' "test('never ends', () => { for (;;) {} });" > calc/test/late.test.js
git -C calc add -A && git -C calc commit -qm "A test that never ends" && git -C calc rev-parse HEAD > late.head && git -C calc checkout -q "$base"
export KERN_CHECK_MINUTES=3
check late "$base"
unset KERN_CHECK_MINUTES
decision late clear
jq -e '.tests.status == "NOT_RUN" and (.tests.why | test("time for this check ran out"))' late/receipt.json > /dev/null || { jq .tests late/receipt.json; fail "late: the run the time limit cut short was not reported as not run"; }

say "Kern runs the tests the way the project's own test script does: its config, its test files and its variables"
git init -q scriptrepo && cd scriptrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p spec src
printf '%s\n' '{"name":"scripted","version":"1.0.0","private":true,"type":"module","scripts":{"test":"FLAVOR=sweet vitest run --config test.config.mjs --reporter=dot"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' "import { defineConfig } from 'vitest/config';" "export default defineConfig({ test: { include: ['spec/**/*.check.js'] } });" > test.config.mjs
printf '%s\n' "export const flavor = () => process.env.FLAVOR;" > src/flavor.js
printf '%s\n' "import { test, expect } from 'vitest';" "import { flavor } from '../src/flavor.js';" '' "test('the flavor comes from the test script', () => expect(flavor()).toBe('sweet'));" > spec/flavor.check.js
git add -A && git commit -qm base && scripted_base=$(git rev-parse HEAD)
printf '%s\n' "export const flavor = () => process.env.FLAVOR.toUpperCase();" > src/flavor.js
git commit -qam "Shout the flavor" && git rev-parse HEAD > "$W/scripted.head"
cd "$W"
check scripted "$scripted_base" scriptrepo
decision scripted blocked
jq -e '.tests.status == "RAN" and (.tests.broke | length == 1)' scripted/receipt.json > /dev/null || { jq .tests scripted/receipt.json; fail "scripted: the project's own test script was not followed"; }

say "Kern check removes its own sandbox volumes"
after=$(volumes)
[ "$after" -le $(( before + 1 )) ] || fail "kern check left $(( after - before )) sandbox volumes behind"

printf '\ne2e-check: PASSED\n'
rm -rf "$W"
