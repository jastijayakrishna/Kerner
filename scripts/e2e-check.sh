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

say "A throwaway verifier (${E2E_RUNTIME:-runsc})"
cd "$W"
if [ "${E2E_RUNTIME:-runsc}" = runc ]; then
  runtime=$(real runc)
  printf '{"host":"e2e-check","deployment":"single-vm","machine":"throwaway","reviewed":"%s"}\n' "$(date -u +%F)" > host-policy.json
else
  runtime=$(readlink -f "$(docker info --format '{{json .Runtimes.runsc}}' | jq -r .path)")
  printf '{"host":"e2e-check","deployment":"single-vm","reviewed":"%s"}\n' "$(date -u +%F)" > host-policy.json
fi
jq -n --arg docker "$(real docker)" --arg git "$(real git)" --arg runtime "$runtime" --arg version "$("$runtime" --version)" --arg policy "$W/host-policy.json" \
  --arg dh "$(hash "$(real docker)")" --arg gh "$(hash "$(real git)")" --arg rh "$(hash "$runtime")" --arg ph "$(hash host-policy.json)" --arg name "${E2E_RUNTIME:-runsc}" \
  '{ version: 1, role: "verifier", docker: { file: $docker, sha256: $dh, endpoint: "unix:///var/run/docker.sock" }, git: { file: $git, sha256: $gh },
     runtime: { file: $runtime, sha256: $rh, version: $version, arguments: [], name: $name }, host_policy: { file: $policy, sha256: $ph } }' > verifier.json
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
  [ "$(jq -r .tests "$1.json")" = RAN ] || echo "$1: tests $(jq -r .tests "$1.json") because $(jq -r '.tests.why // "no reason given"' "$1/receipt.json")"
}
decision() { [ "$(jq -r .decision "$1.json")" = "$2" ] || { cat "$1/receipt.md"; fail "$1: expected $2, got $(jq -r .decision "$1.json")"; }; }
kinds() { jq -r --arg k "$2" '[.'"$3"'[] | select(.kind == $k)] | length' "$1/receipt.json"; }

say "A harmless change is clear, and every test still passes"
check harmless "$base"
[ "$(jq -r .canary harmless.json)" = passed ] || { jq . harmless.json; fail "harmless: Kern's self-test did not come out as known on this machine, so it judged no test"; }
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

say "A check has one time limit, and a run it cuts short is reported as not run: not verified, never a failure or a green check"
git -C calc checkout -q -b late "$base"
printf '%s\n' "import { test } from 'vitest';" '' "test('never ends', () => { for (;;) {} });" > calc/test/late.test.js
git -C calc add -A && git -C calc commit -qm "A test that never ends" && git -C calc rev-parse HEAD > late.head && git -C calc checkout -q "$base"
export KERN_CHECK_MINUTES=3
check late "$base"
unset KERN_CHECK_MINUTES
decision late unverified
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

say "A test script switched to a runner the project does not install is blocked, and the receipt says the runner is missing"
git -C calc checkout -q -b missing "$base"
sed -i 's/"test":"vitest run"/"test":"jest"/' calc/package.json
git -C calc commit -qam "Run the tests with jest" && git -C calc rev-parse HEAD > missing.head && git -C calc checkout -q "$base"
check missing "$base"
decision missing blocked
grep -q 'not installed' missing/receipt.md && [ "$(kinds missing settings look)" = 1 ] || { cat missing/receipt.md; fail "missing: the receipt does not say the runner is missing, or the script change is not a settings item"; }

say "A suite too big for the time limit is checked with the tests related to the change"
git -C calc checkout -q -b big "$base"
printf '%s\n' "import { test } from 'vitest';" '' "test('takes its time', async () => { await new Promise((r) => setTimeout(r, 150000)); }, 200000);" > calc/test/slow.test.js
git -C calc add -A && git -C calc commit -qm "A slow test" && big_base=$(git -C calc rev-parse HEAD)
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' calc/src/total.js
git -C calc commit -qam "Shorter names" && git -C calc rev-parse HEAD > big.head && git -C calc checkout -q "$base"
export KERN_TEST_MINUTES=1
check big "$big_base"
unset KERN_TEST_MINUTES
decision big clear
jq -e '.tests.status == "RAN" and .tests.kept == 3 and (.tests.runner | test("related to the change"))' big/receipt.json > /dev/null || { jq .tests big/receipt.json; fail "big: the related tests did not run in place of the whole suite"; }

say "An edited test whose old version fails only because the value it expects moved is a note with both values, one whose old version expected a refusal the new code no longer makes needs a look, and one whose old version still passes is clear"
git -C calc checkout -q -b rewrites "$base"
sed -i 's/amount >= 100/amount > 100/' calc/src/total.js
sed -i 's/expect(discount(100)).toBe(90)/expect(discount(100)).toBe(100)/' calc/test/total.test.js
git -C calc commit -qam "Discount above 100, and the test says so" && git -C calc rev-parse HEAD > rewrites.head && git -C calc checkout -q "$base"
check rewrites "$base"
decision rewrites clear
[ "$(kinds rewrites test-value notes)" = 1 ] && grep -q 'discount from 100' rewrites/receipt.md && grep -qF 'moves from `90` to `100`' rewrites/receipt.md \
  || { cat rewrites/receipt.md; fail "rewrites: the value the edited test expects moved, and the note does not show both values"; }
git -C calc checkout -q -b guard "$base"
cat >> calc/src/total.js <<'EOF'

export function quantity(n) {
  if (!Number.isInteger(n) || n < 1) throw new Error('a quantity is a whole number from 1');
  return n;
}
EOF
cat > calc/test/guard.test.js <<'EOF'
import { test, expect } from 'vitest';
import { quantity } from '../src/total.js';

test('a zero quantity is refused', () => expect(() => quantity(0)).toThrow());
test('a whole quantity is kept', () => expect(quantity(3)).toBe(3));
EOF
git -C calc add -A && git -C calc commit -qm "Quantities are whole numbers from 1" && guard_base=$(git -C calc rev-parse HEAD)
sed -i "s/throw new Error('a quantity is a whole number from 1');/return 1;/" calc/src/total.js
cat > calc/test/guard.test.js <<'EOF'
import { test, expect } from 'vitest';
import { quantity } from '../src/total.js';

test('a zero quantity becomes 1', () => expect(quantity(0)).toBe(1));
test('a whole quantity is kept', () => expect(quantity(3)).toBe(3));
EOF
git -C calc commit -qam "A bad quantity becomes 1 instead of an error, and the test says so" && git -C calc rev-parse HEAD > guard.head && git -C calc checkout -q "$base"
check guard "$guard_base"
decision guard look
[ "$(kinds guard test-rewritten look)" = 1 ] && grep -q 'accepting what the old code refused' guard/receipt.md \
  || { cat guard/receipt.md; fail "guard: an old test that expected a refusal the new code no longer makes was not a look"; }
git -C calc checkout -q -b edits "$base"
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' calc/src/total.js
sed -i 's/expect(discount(100)).toBe(90)/expect(discount(100)).toEqual(90)/' calc/test/total.test.js
printf '%s\n' "test('discount of 200', () => expect(discount(200)).toBe(180));" >> calc/test/total.test.js
git -C calc commit -qam "Shorter names, one check reworded, and one more test" && git -C calc rev-parse HEAD > edits.head && git -C calc checkout -q "$base"
check edits "$base"
decision edits clear
grep -q 'their old versions still pass on the new code (1 test file)' edits/receipt.md || { cat edits/receipt.md; fail "edits: the old version of the edited test file did not run on the new code"; }

say "In a monorepo whose tests need a workspace package built first, Kern builds it with its own script, on both sides, and then compares"
git init -q builtrepo && cd builtrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p packages/lib/src packages/app/test
printf '%s\n' '{"name":"built","private":true,"type":"module","workspaces":["packages/*"],"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' '{"name":"@mono/lib","version":"1.0.0","type":"module","main":"dist/index.js","scripts":{"build":"node build.mjs"}}' > packages/lib/package.json
printf '%s\n' "import { mkdirSync, copyFileSync } from 'node:fs';" "mkdirSync(new URL('./dist/', import.meta.url), { recursive: true });" "copyFileSync(new URL('./src/index.js', import.meta.url), new URL('./dist/index.js', import.meta.url));" > packages/lib/build.mjs
printf '%s\n' "export const add = (a, b) => a + b;" > packages/lib/src/index.js
printf '%s\n' '{"name":"@mono/app","version":"1.0.0","type":"module","dependencies":{"@mono/lib":"1.0.0"}}' > packages/app/package.json
printf '%s\n' "import { test, expect } from 'vitest';" "import { add } from '@mono/lib';" "" "test('adds', () => expect(add(2, 3)).toBe(5));" > packages/app/test/add.test.js
printf 'dist/\n' > .gitignore
git add -A && git commit -qm base && built_base=$(git rev-parse HEAD)
printf '%s\n' "export const add = (a, b) => a + b + 1;" > packages/lib/src/index.js
git commit -qam "Add one more" && git rev-parse HEAD > "$W/built.head"
cd "$W"
check built "$built_base" builtrepo
decision built blocked
jq -e '.tests.status == "RAN" and (.tests.broke | length == 1) and (.tests.runner | test("packages/lib build"))' built/receipt.json > /dev/null || { jq .tests built/receipt.json; fail "built: the workspace package was not built before the tests ran"; }

say "Tests that need a file made by the pretest script, in a suite too big for the time limit, are prepared first and then checked with the tests related to the change"
git init -q genrepo && cd genrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test scripts
printf '%s\n' '{"name":"gen","version":"1.0.0","private":true,"type":"module","scripts":{"pretest":"node scripts/gen.mjs","test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' 'import { writeFileSync } from "node:fs";' 'writeFileSync(new URL("../src/version.js", import.meta.url), "export const version = 1;\n");' > scripts/gen.mjs
printf '%s\n' 'export default { test: { setupFiles: ["./test/setup.js"] } };' > vitest.config.js
printf '%s\n' 'import "../src/version.js";' > test/setup.js
printf '%s\n' 'export const total = (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0);' > src/total.js
printf '%s\n' 'import { test, expect } from "vitest";' 'import { total } from "../src/total.js";' '' 'test("adds up a basket", () => expect(total([{ price: 2, qty: 3 }, { price: 1, qty: 4 }])).toBe(10));' > test/total.test.js
printf '%s\n' 'import { test } from "vitest";' '' 'test("takes its time", async () => { await new Promise((r) => setTimeout(r, 150000)); }, 200000);' > test/slow.test.js
printf 'src/version.js\n' > .gitignore
git add -A && git commit -qm base && gen_base=$(git rev-parse HEAD)
sed -i 's/sum + item.price \* item.qty/sum + item.price * item.qty + 1/' src/total.js
git commit -qam "Count one more" && git rev-parse HEAD > "$W/gen.head"
cd "$W"
export KERN_TEST_MINUTES=1
check gen "$gen_base" genrepo
unset KERN_TEST_MINUTES
decision gen blocked
jq -e '.tests.status == "RAN" and (.tests.broke | length == 1) and (.tests.runner | test("pretest")) and (.tests.runner | test("related to the change"))' gen/receipt.json > /dev/null || { jq .tests gen/receipt.json; fail "gen: the pretest script did not run first, or the related tests did not run in place of the whole suite"; }

say "A project that asks for another Node, ends its test script with --, runs an old vitest and edits a copy of its fixture is installed and tested all the same"
git init -q strictrepo && cd strictrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test/fixtures
printf '%s\n' '{"name":"strict","version":"1.0.0","private":true,"type":"module","engines":{"node":"18.x"},"scripts":{"test":"vitest run --"},"devDependencies":{"vitest":"0.34.6"}}' > package.json
printf 'engine-strict=true\n' > .npmrc
printf '%s\n' 'export const add = (a, b) => a + b;' > src/add.js
printf '{"count":1}\n' > test/fixtures/state.json
printf '%s\n' 'import { test, expect } from "vitest";' 'import { cpSync, writeFileSync, readFileSync, mkdtempSync } from "node:fs";' 'import { tmpdir } from "node:os";' 'import { join } from "node:path";' 'import { add } from "../src/add.js";' '' \
  'test("adds", () => expect(add(2, 3)).toBe(5));' \
  'test("edits a copy of its fixture", () => {' '  const dir = mkdtempSync(join(tmpdir(), "strict-"));' '  cpSync(new URL("./fixtures/state.json", import.meta.url), join(dir, "state.json"));' \
  '  writeFileSync(join(dir, "state.json"), "{\"count\":2}");' '  expect(JSON.parse(readFileSync(join(dir, "state.json"), "utf8")).count).toBe(2);' '});' > test/add.test.js
git add -A && git commit -qm base && strict_base=$(git rev-parse HEAD)
sed -i 's/a + b/a + b + 1/' src/add.js
git commit -qam "Add one more" && git rev-parse HEAD > "$W/strict.head"
cd "$W"
check strict "$strict_base" strictrepo
decision strict blocked
jq -e '.tests.status == "RAN" and (.tests.broke | length == 1) and .tests.before.passed == 2 and (.tests.already_failing | length == 0)' strict/receipt.json > /dev/null || { jq .tests strict/receipt.json; jq . strict/setup.json 2>/dev/null; fail "strict: the project was not installed and tested as it is, or its fixture test could not edit its own copy"; }

say "A runner that stops before running any test is quoted, not reported as finding no tests"
git init -q crashrepo && cd crashrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"crash","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' 'export default { test: { globalSetup: ["./test/setup.js"] } };' > vitest.config.js
printf '%s\n' 'export default () => { if (!process.env.SHOP_DATABASE_URL) throw new Error("SHOP_DATABASE_URL must be set to a test database"); };' > test/setup.js
printf '%s\n' 'export const a = 1;' > src/a.js
printf '%s\n' 'import { test, expect } from "vitest";' 'import { a } from "../src/a.js";' '' 'test("a", () => expect(a).toBe(1));' > test/a.test.js
git add -A && git commit -qm base && crash_base=$(git rev-parse HEAD)
sed -i 's/a = 1/a = 2/' src/a.js
git commit -qam "Two" && git rev-parse HEAD > "$W/crash.head"
cd "$W"
check crash "$crash_base" crashrepo
jq -e '.tests.status == "NOT_RUN" and (.tests.why | test("SHOP_DATABASE_URL must be set"))' crash/receipt.json > /dev/null || { jq .tests crash/receipt.json; fail "crash: the runner's own error was not quoted"; }

say "A suite on course to take far longer than its limit is stopped early and checked with the tests related to the change, with vitest and with jest"
for runner in vitest jest; do
  git init -q "early-$runner-repo" && cd "early-$runner-repo"
  git config user.email e2e@example.invalid && git config user.name e2e
  mkdir -p src test
  if [ "$runner" = vitest ]; then
    printf '%s\n' '{"name":"early","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
    printf '%s\n' 'export default { test: { fileParallelism: false } };' > vitest.config.js
    printf '%s\n' 'export const total = (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0);' > src/total.js
    printf '%s\n' 'import { test, expect } from "vitest";' 'import { total } from "../src/total.js";' '' 'test("total", () => expect(total([{ price: 2, qty: 3 }])).toBe(6));' > test/total.test.js
    slow='import { test } from "vitest";'
  else
    printf '%s\n' '{"name":"early","version":"1.0.0","private":true,"scripts":{"test":"jest --maxWorkers=1"},"devDependencies":{"jest":"29.7.0"}}' > package.json
    printf '%s\n' 'module.exports = { total: (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0) };' > src/total.js
    printf '%s\n' 'const { total } = require("../src/total.js");' '' 'test("total", () => expect(total([{ price: 2, qty: 3 }])).toBe(6));' > test/total.test.js
    slow=''
  fi
  for i in $(seq 1 120); do printf '%s\n' "$slow" 'test("waits", async () => { await new Promise((r) => setTimeout(r, 3000)); }, 20000);' > "test/slow-$i.test.js"; done
  git add -A && git commit -qm base && early_base=$(git rev-parse HEAD)
  sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' src/total.js
  git commit -qam "Shorter names" && git rev-parse HEAD > "$W/early-$runner.head"
  cd "$W"
  export KERN_TEST_MINUTES=1
  check "early-$runner" "$early_base" "early-$runner-repo"
  unset KERN_TEST_MINUTES
  decision "early-$runner" clear
  jq -e '.tests.status == "RAN" and (.tests.runner | test("related to the change: the whole suite was on course to take about"))' "early-$runner/receipt.json" > /dev/null \
    || { jq .tests "early-$runner/receipt.json"; fail "early-$runner: the run was not stopped early, so its progress was not read"; }
done

say "A suite whose config stops at its first failure is still run whole, so a test the change breaks is found"
git init -q bailrepo && cd bailrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"bail","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' 'export default { test: { bail: process.env.CI ? 1 : 0, fileParallelism: false, sequence: { shuffle: true } } };' > vitest.config.js
printf '%s\n' 'export const total = (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0);' > src/total.js
printf '%s\n' 'import { test, expect } from "vitest";' "const padding = \"$(printf 'x%.0s' $(seq 1 600))\";" '' 'test("needs a service that is not here", () => expect(padding.length + process.env.NOT_HERE).toBe("600yes"));' > test/a-service.test.js
for i in 1 2 3; do printf '%s\n' 'import { test, expect } from "vitest";' 'import { total } from "../src/total.js";' '' "test(\"total $i\", () => expect(total([{ price: $i, qty: 2 }])).toBe($(( i * 2 ))));" > "test/total-$i.test.js"; done
git add -A && git commit -qm base && bail_base=$(git rev-parse HEAD)
sed -i 's/sum + item.price \* item.qty/sum + item.price * item.qty + 1/' src/total.js
git commit -qam "Count one more" && git rev-parse HEAD > "$W/bail.head"
cd "$W"
check bail "$bail_base" bailrepo
decision bail blocked
jq -e '.tests.status == "RAN" and .tests.before.passed == 3 and (.tests.broke | length == 3)' bail/receipt.json > /dev/null || { jq .tests bail/receipt.json; fail "bail: the suite stopped at its first failure, so the broken tests were not all found"; }

say "A build Kern runs for one package's tests is run on both sides, so a later package never sees it on one side only"
git init -q mirrorrepo && cd mirrorrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p packages/lib/src packages/ghost packages/a/test packages/a/src packages/b/test packages/b/src
printf '%s\n' '{"name":"mirror","private":true,"type":"module","workspaces":["packages/*"],"scripts":{"test":"npm run test --workspaces"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' '{"name":"@m/lib","version":"1.0.0","type":"module","main":"dist/index.js","scripts":{"build":"node build.mjs"}}' > packages/lib/package.json
printf '%s\n' "import { mkdirSync, copyFileSync } from 'node:fs';" "mkdirSync(new URL('./dist/', import.meta.url), { recursive: true });" "copyFileSync(new URL('./src/index.js', import.meta.url), new URL('./dist/index.js', import.meta.url));" > packages/lib/build.mjs
printf '%s\n' "export const add = (a, b) => a + b;" > packages/lib/src/index.js
printf '%s\n' '{"name":"@m/ghost","version":"1.0.0","type":"module","main":"dist/index.js","dependencies":{"@m/lib":"1.0.0"}}' > packages/ghost/package.json
for p in a b; do printf '%s\n' "{\"name\":\"@m/$p\",\"version\":\"1.0.0\",\"type\":\"module\",\"scripts\":{\"test\":\"vitest run\"},\"dependencies\":{\"@m/lib\":\"1.0.0\",\"@m/ghost\":\"1.0.0\"}}" > "packages/$p/package.json"; printf '%s\n' "export const name = '$p';" > "packages/$p/src/name.js"; done
printf '%s\n' "import { test, expect } from 'vitest';" "import { add } from '@m/lib';" "import { ghost } from '@m/ghost';" '' "test('adds', () => expect(add(2, 3) + ghost).toBe(5));" > packages/a/test/add.test.js
printf '%s\n' "import { test, expect } from 'vitest';" "import { add } from '@m/lib';" '' "test('adds', () => expect(add(2, 3)).toBe(5));" > packages/b/test/add.test.js
printf 'dist/\n' > .gitignore
git add -A && git commit -qm base && mirror_base=$(git rev-parse HEAD)
sed -i "s/'a'/'aa'/" packages/a/src/name.js && sed -i "s/'b'/'bb'/" packages/b/src/name.js
git commit -qam "Longer names" && git rev-parse HEAD > "$W/mirror.head"
cd "$W"
check mirror "$mirror_base" mirrorrepo
jq -e '.decision != "blocked" and (.tests.runner | test("vitest in packages/b"))' mirror/receipt.json > /dev/null || { jq '{decision, tests}' mirror/receipt.json; fail "mirror: a build run for packages/a on the old side only made packages/b look broken on the new side"; }

say "Tests that exist only after the test script's own build step are built first, on both sides"
git init -q compiledrepo && cd compiledrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"compiled","version":"1.0.0","private":true,"type":"module","scripts":{"build":"node build.mjs","test":"npm run build && mocha"},"devDependencies":{"mocha":"10.7.3"}}' > package.json
printf '%s\n' '{"spec":"out/test/*.test.js"}' > .mocharc.json
printf '%s\n' 'import { mkdirSync, cpSync } from "node:fs";' 'mkdirSync(new URL("./out/", import.meta.url), { recursive: true });' \
  'cpSync(new URL("./src/", import.meta.url), new URL("./out/src/", import.meta.url), { recursive: true });' \
  'cpSync(new URL("./test/", import.meta.url), new URL("./out/test/", import.meta.url), { recursive: true });' > build.mjs
printf '%s\n' 'export const total = (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0);' > src/total.js
printf '%s\n' 'import assert from "node:assert";' 'import { total } from "../src/total.js";' '' 'it("total", () => assert.equal(total([{ price: 2, qty: 3 }]), 6));' > test/total.test.js
printf 'out/\n' > .gitignore
git add -A && git commit -qm base && compiled_base=$(git rev-parse HEAD)
sed -i 's/sum + item.price \* item.qty/sum + item.price * item.qty + 1/' src/total.js
git commit -qam "Count one more" && git rev-parse HEAD > "$W/compiled.head"
cd "$W"
check compiled "$compiled_base" compiledrepo
decision compiled blocked
jq -e '.tests.status == "RAN" and (.tests.broke | length == 1) and (.tests.runner | test("after Kern ran \\. build"))' compiled/receipt.json > /dev/null || { jq .tests compiled/receipt.json; fail "compiled: the test script's build step was not run before its tests"; }

say "In an nx-style monorepo, a changed project with no test script runs the runner its own config names"
git init -q nxrepo && cd nxrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p libs/calc/src
printf '%s\n' '{"name":"nxlike","private":true,"scripts":{"test":"nx run-many -t test"},"devDependencies":{"jest":"29.7.0"}}' > package.json
printf '%s\n' '{"name":"calc","targets":{"test":{"executor":"@nx/jest:jest","options":{"jestConfig":"libs/calc/jest.config.js"}}}}' > libs/calc/project.json
printf '%s\n' 'module.exports = { testEnvironment: "node" };' > libs/calc/jest.config.js
printf '%s\n' 'module.exports = { total: (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0) };' > libs/calc/src/total.js
printf '%s\n' 'const { total } = require("./total.js");' '' 'test("total", () => expect(total([{ price: 2, qty: 3 }])).toBe(6));' > libs/calc/src/total.test.js
git add -A && git commit -qm base && nx_base=$(git rev-parse HEAD)
sed -i 's/sum + item.price \* item.qty/sum + item.price * item.qty + 1/' libs/calc/src/total.js
git commit -qam "Count one more" && git rev-parse HEAD > "$W/nx.head"
cd "$W"
check nx "$nx_base" nxrepo
decision nx blocked
jq -e '.tests.status == "RAN" and (.tests.broke | length == 1) and (.tests.runner | test("jest in libs/calc"))' nx/receipt.json > /dev/null || { jq .tests nx/receipt.json; fail "nx: the project's own jest config was not run"; }

say "When only the new code's install fails, the receipt quotes the install as not verified instead of blaming the tests, and the new package that cannot install is listed as a note"
git -C calc checkout -q -b installfail "$base"
sed -i 's/"devDependencies":{"vitest":"3.2.4"}/"devDependencies":{"vitest":"3.2.4","@kern-e2e\/does-not-exist":"1.0.0"}/' calc/package.json
git -C calc commit -qam "Add a package that does not exist" && git -C calc rev-parse HEAD > installfail.head && git -C calc checkout -q "$base"
check installfail "$base"
decision installfail unverified
[ "$(kinds installfail dependencies notes)" = 1 ] && [ "$(kinds installfail dependencies look)" = 0 ] && [ "$(kinds installfail test-unconfirmed unverified)" = 1 ] && [ "$(kinds installfail tests-not-run unverified)" = 0 ] \
  || { cat installfail/receipt.md; fail "installfail: the new package is not a note, or the failed install is not listed once as not verified"; }
jq -e '(.tests.run_unconfirmed | length == 1) and (.tests.run_unconfirmed[0].why | test("installing its dependencies with npm failed"))' installfail/receipt.json > /dev/null || { jq .tests installfail/receipt.json; fail "installfail: a failed install on the new code was not named, or it blocked"; }

say "When only some test files miss an unbuilt workspace package, Kern builds it on both sides and compares them all"
git init -q partialrepo && cd partialrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p packages/lib/src packages/app/test
printf '%s\n' '{"name":"partial","private":true,"type":"module","workspaces":["packages/*"],"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' '{"name":"@p/lib","version":"1.0.0","type":"module","main":"dist/index.js","scripts":{"build":"node build.mjs"}}' > packages/lib/package.json
printf '%s\n' "import { mkdirSync, copyFileSync } from 'node:fs';" "mkdirSync(new URL('./dist/', import.meta.url), { recursive: true });" "copyFileSync(new URL('./src/index.js', import.meta.url), new URL('./dist/index.js', import.meta.url));" > packages/lib/build.mjs
printf '%s\n' "export const add = (a, b) => a + b;" > packages/lib/src/index.js
printf '%s\n' '{"name":"@p/app","version":"1.0.0","type":"module","dependencies":{"@p/lib":"1.0.0"}}' > packages/app/package.json
printf '%s\n' "import { test, expect } from 'vitest';" "import { add } from '@p/lib';" '' "test('adds', () => expect(add(2, 3)).toBe(5));" > packages/app/test/add.test.js
printf '%s\n' "import { test, expect } from 'vitest';" '' "test('stands alone', () => expect(1 + 1).toBe(2));" > packages/app/test/alone.test.js
printf 'dist/\n' > .gitignore
git add -A && git commit -qm base && partial_base=$(git rev-parse HEAD)
printf '%s\n' "export const add = (a, b) => a + b + 1;" > packages/lib/src/index.js
git commit -qam "Add one more" && git rev-parse HEAD > "$W/partial.head"
cd "$W"
check partial "$partial_base" partialrepo
decision partial blocked
jq -e '.tests.status == "RAN" and .tests.before.passed == 2 and (.tests.broke | length == 1) and (.tests.runner | test("packages/lib build"))' partial/receipt.json > /dev/null \
  || { jq .tests partial/receipt.json; fail "partial: a test file that missed an unbuilt package, while another passed, was set aside instead of built for"; }

say "When a test needs two unbuilt workspace packages and the runner names only the first, Kern builds the second on the next pass, as lightdash/lightdash#12942 needed"
git init -q layersrepo && cd layersrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p packages/lib/src packages/fmt/src packages/app/test
printf '%s\n' '{"name":"layers","private":true,"type":"module","workspaces":["packages/*"],"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
for pkg in lib fmt; do
  printf '{"name":"@p/%s","version":"1.0.0","type":"module","main":"dist/index.js","scripts":{"build":"node build.mjs"}}\n' "$pkg" > "packages/$pkg/package.json"
  printf '%s\n' "import { mkdirSync, copyFileSync } from 'node:fs';" "mkdirSync(new URL('./dist/', import.meta.url), { recursive: true });" "copyFileSync(new URL('./src/index.js', import.meta.url), new URL('./dist/index.js', import.meta.url));" > "packages/$pkg/build.mjs"
done
printf '%s\n' "export const add = (a, b) => a + b;" > packages/lib/src/index.js
printf '%s\n' "export const fmt = (n) => 'n=' + n;" > packages/fmt/src/index.js
printf '%s\n' '{"name":"@p/app","version":"1.0.0","type":"module","dependencies":{"@p/lib":"1.0.0","@p/fmt":"1.0.0"}}' > packages/app/package.json
printf '%s\n' "import { test, expect } from 'vitest';" "import { add } from '@p/lib';" "import { fmt } from '@p/fmt';" '' "test('adds', () => expect(fmt(add(2, 3))).toBe('n=5'));" > packages/app/test/add.test.js
printf '%s\n' "import { test, expect } from 'vitest';" '' "test('stands alone', () => expect(1 + 1).toBe(2));" > packages/app/test/alone.test.js
printf 'dist/\n' > .gitignore
git add -A && git commit -qm base && layers_base=$(git rev-parse HEAD)
printf '%s\n' "export const add = (a, b) => a + b + 1;" > packages/lib/src/index.js
git commit -qam "Add one more" && git rev-parse HEAD > "$W/layers.head"
cd "$W"
check layers "$layers_base" layersrepo
decision layers blocked
jq -e '.tests.status == "RAN" and .tests.before.passed == 2 and (.tests.broke | length == 1) and (.tests.runner | test("packages/lib build")) and (.tests.runner | test("packages/fmt build"))' layers/receipt.json > /dev/null \
  || { jq .tests layers/receipt.json; fail "layers: a second unbuilt package the runner named only after the first was built was never built"; }

say "A change that breaks the project's own build while its tests still pass needs a look that quotes the build's error, as planyr#1262 needed; a build that still works gets its check mark"
git init -q buildrepo && cd buildrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"buildcheck","version":"1.0.0","private":true,"type":"module","scripts":{"test":"node --test","build":"node build.mjs"}}' > package.json
printf '%s\n' "import { readdirSync } from 'node:fs';" "for (const f of readdirSync(new URL('./src/', import.meta.url))) await import(new URL('./src/' + f, import.meta.url));" "console.log('built');" > build.mjs
printf '%s\n' "export const add = (a, b) => a + b;" > src/index.js
printf '%s\n' "import { test } from 'node:test';" "import assert from 'node:assert';" "import { add } from '../src/index.js';" '' "test('adds', () => assert.equal(add(2, 3), 5));" > test/index.test.js
git add -A && git commit -qm base && build_base=$(git rev-parse HEAD)
git checkout -q -b broken
printf '%s\n' "import { subtract } from './index.js';" "export const report = (a, b) => subtract(a, b);" > src/report.js
git add -A && git commit -qm "Add a report" && git rev-parse HEAD > "$W/buildbroken.head"
git checkout -q -b fine "$build_base"
printf '%s\n' "export const add = (a, b) => b + a;" > src/index.js
git commit -qam "Swap the operands" && git rev-parse HEAD > "$W/buildfine.head"
git checkout -q "$build_base"
cd "$W"
check buildbroken "$build_base" buildrepo
decision buildbroken look
jq -e '.tests.status == "RAN" and .tests.builds.builds[0].result == "broke" and ([.look[].kind] | index("build-broke") != null)' buildbroken/receipt.json > /dev/null \
  || { jq '.tests.builds, .look' buildbroken/receipt.json; fail "buildbroken: a build the change broke was not named"; }
grep -q "subtract" buildbroken/receipt.md || { cat buildbroken/receipt.md; fail "buildbroken: the receipt does not quote the build's error"; }
check buildfine "$build_base" buildrepo
decision buildfine clear
grep -q '✅ Build: `npm run build` works on the old and the new code' buildfine/receipt.md || { cat buildfine/receipt.md; fail "buildfine: a build that still works has no check mark"; }

say "Inputs the AI step proposed run through the project's own runner on the old and the new code: a call that now throws against a verified requirement that it must not is blocked with the input and both answers; an answer that changed against a requirement that it must not change is a look, as civitai#3102 needed; inputs whose answers stay the same get their check mark"
git init -q proberepo && cd proberepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"probe","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
cat > src/sql.js <<'EOF'
export function removeComments(sql) {
  let out = '';
  let i = 0;
  while (i < sql.length) {
    if (sql.startsWith('$$', i)) { const end = sql.indexOf('$$', i + 2); const stop = end < 0 ? sql.length : end + 2; out += sql.slice(i, stop); i = stop; continue; }
    if (sql[i] === "'") { const end = sql.indexOf("'", i + 1); const stop = end < 0 ? sql.length : end + 1; out += sql.slice(i, stop); i = stop; continue; }
    if (sql.startsWith('--', i) || sql.startsWith('//', i)) { const end = sql.indexOf('\n', i); i = end < 0 ? sql.length : end; continue; }
    out += sql[i++];
  }
  return out;
}
EOF
cat > test/sql.test.js <<'EOF'
import { test, expect } from 'vitest';
import { removeComments } from '../src/sql.js';

test('keeps -- inside quoted strings', () => expect(removeComments("SELECT '--x' AS a -- note")).toBe("SELECT '--x' AS a "));
EOF
git add -A && git commit -qm base && probe_base=$(git rev-parse HEAD)
git checkout -q -b lost
grep -vF "if (sql.startsWith('\$\$', i))" src/sql.js > src/sql.next && mv src/sql.next src/sql.js
git commit -qam "One kind of quote is enough" && git rev-parse HEAD > "$W/probelost.head"
git checkout -q -b kept "$probe_base"
sed -i 's/out += sql\[i++\];/out = out + sql[i++];/' src/sql.js
git commit -qam "Spell out the append" && git rev-parse HEAD > "$W/probekept.head"
git checkout -q -b throws "$probe_base"
sed -i "s/if (sql.startsWith('\$\$', i)) {.*/if (sql.startsWith('\$\$', i)) { throw new TypeError('dollar quoting is not supported'); }/" src/sql.js
git commit -qam "Refuse dollar quoting" && git rev-parse HEAD > "$W/probethrows.head"
git checkout -q "$probe_base"
cd "$W"
[ "$(git -C proberepo diff --stat "$probe_base" "$(cat probelost.head)" | tail -1 | grep -c '1 deletion')" = 1 ] && [ "$(git -C proberepo diff --stat "$probe_base" "$(cat probekept.head)" | tail -1 | grep -c '1 insertion(+), 1 deletion')" = 1 ] \
  && [ "$(git -C proberepo diff --stat "$probe_base" "$(cat probethrows.head)" | tail -1 | grep -c '1 insertion(+), 1 deletion')" = 1 ] \
  || fail "proberepo: each change should edit exactly one line of src/sql.js"
investigation() {
  jq -n --arg base "$probe_base" --arg head "$(cat "$W/$1.head")" --arg check "$2" --arg quote "test('keeps -- inside quoted strings'" \
    '{ version: 1, base: $base, head: $head, status: "INVESTIGATED", why: "", model: "fixture", usage: { input_tokens: 0, output_tokens: 0 },
       targets: [{ path: "src/sql.js", export: "removeComments", shapes: [["string"]], retested: false }],
       obligations: [{ id: "o1", statement: (if $check == "same" then "Removing comments keeps the text inside quoted strings unchanged." else "Removing comments never throws for a query string." end), check: $check,
         targets: [{ path: "src/sql.js", export: "removeComments" }], source: { path: "test/sql.test.js", quote: $quote, kind: "test" }, authority: "verified", why: "", authorized_by: "" }],
       experiments: [{ id: "e1", obligation: "o1", path: "src/sql.js", export: "removeComments", args: ["SELECT $$https://example.com/a//b$$ AS address"], why: "a dollar-quoted string that holds //" },
         { id: "e2", obligation: "o1", path: "src/sql.js", export: "removeComments", args: ["SELECT 1 -- note"], why: "a plain comment" }], dropped: [] }' > "$W/$1.investigation.json"
}
investigation probelost same
investigation probekept same
investigation probethrows returns
for c in probelost probekept probethrows; do
  export KERN_INVESTIGATION="$W/$c.investigation.json"
  check "$c" "$probe_base" proberepo
  unset KERN_INVESTIGATION
done
[ "$(jq -r .decision probethrows.json)" = blocked ] && [ "$(kinds probethrows probe-regression blocked)" = 1 ] && grep -qF 'removeComments("SELECT $$https://example.com/a//b$$ AS address")' probethrows/receipt.md \
  && grep -qF '"SELECT $$https://example.com/a//b$$ AS address"' probethrows/receipt.md && grep -qF 'TypeError: dollar quoting is not supported' probethrows/receipt.md \
  || { cat probethrows/receipt.md; jq .tests.probes probethrows/receipt.json; cat probethrows/trace-probe-*.err 2>/dev/null | tail -n 40; fail "probethrows: the input that now throws was not blocked with the input and both answers"; }
[ "$(jq -r .decision probelost.json)" = look ] && [ "$(kinds probelost probe-concern look)" = 1 ] && grep -qF 'removeComments("SELECT $$https://example.com/a//b$$ AS address")' probelost/receipt.md \
  && grep -qF '"SELECT $$https:"' probelost/receipt.md && grep -qF 'covers the inputs its source names' probelost/receipt.md \
  || { cat probelost/receipt.md; jq .tests.probes probelost/receipt.json; cat probelost/trace-probe-*.err 2>/dev/null | tail -n 40; fail "probelost: the input that truncates a dollar-quoted string was not a look with the input, both answers and why it is no block"; }
[ "$(jq -r .decision probekept.json)" = clear ] && grep -q '✅ Probes: 2 inputs the AI step chose, for 1 function, got the same answers on the old and the new code$' probekept/receipt.md \
  || { cat probekept/receipt.md; jq .tests.probes probekept/receipt.json; cat probekept/trace-probe-*.err 2>/dev/null | tail -n 40; fail "probekept: inputs whose answers stay the same have no check mark"; }

say "Kern undoes each changed function and plants small faults in the changed lines: a same-behaviour rewrite the tests guard counts as checked, and a change no test checks becomes a task for the agent, with vitest and with jest"
for runner in vitest jest; do
  git init -q "faults-$runner-repo" && cd "faults-$runner-repo"
  git config user.email e2e@example.invalid && git config user.name e2e
  mkdir -p src test
  if [ "$runner" = vitest ]; then
    printf '%s\n' '{"name":"faults","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
    def() { printf 'export const %s = %s;\n' "$1" "$2"; }
    printf '%s\n' 'import { test, expect } from "vitest";' 'import { discount } from "../src/price.js";' '' > test/price.test.js
  else
    printf '%s\n' '{"name":"faults","version":"1.0.0","private":true,"scripts":{"test":"jest"},"devDependencies":{"jest":"29.7.0"}}' > package.json
    def() { printf 'exports.%s = %s;\n' "$1" "$2"; }
    printf '%s\n' 'const { discount } = require("../src/price.js");' '' > test/price.test.js
  fi
  printf '%s\n' 'test("discount from 100", () => expect(discount(100)).toBe(90));' 'test("no discount below 100", () => expect(discount(99)).toBe(99));' >> test/price.test.js
  { def discount '(amount) => (amount >= 100 ? amount * 0.9 : amount)'; def free '(total) => total >= 50'; } > src/price.js
  def shipping '(weight) => (weight > 10 ? 15 : 5)' > src/ship.js
  git add -A && git commit -qm base && faults_base=$(git rev-parse HEAD)
  git checkout -q -b some
  { def discount '(amount) => (amount < 100 ? amount : amount * 0.9)'; def free '(total) => total > 60'; } > src/price.js
  git commit -qam "Same discount, written the other way round, and free shipping from 60" && git rev-parse HEAD > "$W/faults-some-$runner.head"
  git checkout -q -b none "$faults_base"
  { def discount '(amount) => (amount >= 100 ? amount * 0.9 : amount)'; def free '(total) => total > 60'; } > src/price.js
  def shipping '(weight) => (weight > 20 ? 15 : 5)' > src/ship.js
  git commit -qam "Free shipping from 60, and heavy parcels from 20" && git rev-parse HEAD > "$W/faults-none-$runner.head"
  git checkout -q "$faults_base"
  cd "$W"
  check "faults-some-$runner" "$faults_base" "faults-$runner-repo"
  decision "faults-some-$runner" clear
  jq -e '.tests.status == "RAN" and ([.tests.faults[] | "\(.path):\(.line) \(.outcome)"] | sort == ["src/price.js:1 caught", "src/price.js:1 missed", "src/price.js:2 missed", "src/price.js:2 missed"])
    and ([.challenges[] | "\(.target) \(.kind)"] == ["free unasserted"])' "faults-some-$runner/receipt.json" > /dev/null \
    || { jq .tests "faults-some-$runner/receipt.json"; fail "faults-some-$runner: the fault a test notices was not caught, or the one no test checks was not missed"; }
  grep -q 'the tests caught 1 of 2 small faults' "faults-some-$runner/receipt.md" && grep -q 'Checked by a test: 1 of 2 changed functions' "faults-some-$runner/receipt.md" && grep -q 'For the agent that wrote this change' "faults-some-$runner/receipt.md" && grep -q 'src/price.js:2' "faults-some-$runner/receipt.md" \
    || { cat "faults-some-$runner/receipt.md"; fail "faults-some-$runner: the receipt does not say which planted fault no test noticed"; }
  check "faults-none-$runner" "$faults_base" "faults-$runner-repo"
  decision "faults-none-$runner" clear
  jq -e '([.tests.faults[] | "\(.path):\(.line) \(.outcome)"] | sort) == ["src/price.js:2 missed", "src/price.js:2 missed", "src/ship.js:1 untested", "src/ship.js:1 untested"]
    and ([.challenges[] | "\(.target) \(.kind)"] | sort) == ["free unasserted", "shipping unreached"]' "faults-none-$runner/receipt.json" > /dev/null \
    || { jq .tests "faults-none-$runner/receipt.json"; fail "faults-none-$runner: a change no test checks was not found"; }
  grep -q 'the tests caught 0 of 2 small faults' "faults-none-$runner/receipt.md" || { cat "faults-none-$runner/receipt.md"; fail "faults-none-$runner: the receipt does not say no test checks the change"; }
done

say "Kern watches what the changed code gives tests the change leaves untouched: an answer they never checked is named as a note, with vitest and with jest"
for runner in vitest jest; do
  git init -q "watch-$runner-repo" && cd "watch-$runner-repo"
  git config user.email e2e@example.invalid && git config user.name e2e
  mkdir -p src test
  if [ "$runner" = vitest ]; then
    printf '%s\n' '{"name":"watch","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
    def() { printf 'export const %s = %s;\n' "$1" "$2"; }
    printf '%s\n' 'import { test, expect } from "vitest";' 'import { label } from "../src/label.js";' '' > test/label.test.js
  else
    printf '%s\n' '{"name":"watch","version":"1.0.0","private":true,"scripts":{"test":"jest"},"devDependencies":{"jest":"29.7.0"}}' > package.json
    def() { printf 'exports.%s = %s;\n' "$1" "$2"; }
    printf '%s\n' 'const { label } = require("../src/label.js");' '' > test/label.test.js
  fi
  printf '%s\n' 'test("labels one item", () => expect(label(1)).toBe("1 item"));' 'test("labels many", () => expect(label(3).length).toBeGreaterThan(0));' >> test/label.test.js
  if [ "$runner" = vitest ]; then
    printf '%s\n' 'import { label } from "./label.js";' 'export const summary = (n) => "Total: " + label(n);' > src/report.js
    printf '%s\n' 'import { test, expect } from "vitest";' 'import { summary } from "../src/report.js";' 'test("summarises", () => expect(summary(2).length).toBeGreaterThan(0));' > test/report.test.js
  else
    printf '%s\n' 'const { label } = require("./label.js");' 'exports.summary = (n) => "Total: " + label(n);' > src/report.js
    printf '%s\n' 'const { summary } = require("../src/report.js");' 'test("summarises", () => expect(summary(2).length).toBeGreaterThan(0));' > test/report.test.js
  fi
  def label '(n) => (n === 1 ? "1 item" : n + " items")' > src/label.js
  git add -A && git commit -qm base && watch_base=$(git rev-parse HEAD)
  git checkout -q -b things
  def label '(n) => (n === 1 ? "1 item" : n + " things")' > src/label.js
  git commit -qam "Say things instead of items" && git rev-parse HEAD > "$W/watch-$runner.head"
  git checkout -q "$watch_base"
  cd "$W"
  check "watch-$runner" "$watch_base" "watch-$runner-repo"
  decision "watch-$runner" clear
  jq -c '.tests.map' "watch-$runner/receipt.json"; tail -n 5 "watch-$runner/map.err" 2> /dev/null; grep -h 'Map' "watch-$runner/receipt.md"
  jq -e '.tests.bystanders.status == "RAN" and .tests.bystanders.changed == 3
    and ([.tests.bystanders.differences[] | select(.key == "src/label.js#label" and .input == "[3]") | .now] | any(contains("3 things"))) and ([.notes[].kind] | index("bystander") != null)
    and ([.tests.bystanders.differences[] | select(.key == "src/report.js#summary") | .now] | any(contains("2 things")))
    and ([.challenges[] | "\(.target) \(.kind)"] == ["label unasserted"]) and (.challenges[0].example | test("label\\([23]\\)"))' "watch-$runner/receipt.json" > /dev/null \
    || { jq '.tests.bystanders, .notes' "watch-$runner/receipt.json"; fail "watch-$runner: the answer the untouched test never checked was not named, or the house did not watch src/report.js, which the change reaches"; }
  if [ "$runner" = vitest ]; then map_edit='.tests.map.changed_functions == 1 and .tests.map.files_running == 1'; else map_edit='.tests.map.changed_modules == 1 and .tests.map.files_loading == 1'; fi
  jq -e ".tests.map.status == \"RAN\" and $map_edit and .tests.map.nearest == \"src/report.js\" and .tests.map.test_files == 2 and .tests.map.complete" "watch-$runner/receipt.json" > /dev/null \
    || { jq '.tests.map' "watch-$runner/receipt.json"; tail -n 20 "watch-$runner/map.err" 2> /dev/null; grep -h 'Map' "watch-$runner/receipt.md"; fail "watch-$runner: the compiler-backed map was not built in the sandbox, or it did not say that src/report.js runs label() (inside summary() with vitest; when it loads with jest, whose exports.label the index sees only as the module) and two test files call it"; }
  grep -q '🔍 Untouched tests' "watch-$runner/receipt.md" || { cat "watch-$runner/receipt.md"; fail "watch-$runner: the receipt does not say what the untouched tests saw"; }
done

say "A change that brings TypeScript errors to a project that type-checked clean is blocked, even when its tests still pass"
git init -q typesrepo && cd typesrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"typed","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run","typecheck":"tsc --noEmit"},"devDependencies":{"vitest":"3.2.4","typescript":"5.6.3"}}' > package.json
printf '%s\n' '{ "compilerOptions": { "strict": true, "target": "ES2022", "module": "ESNext", "moduleResolution": "Bundler", "noEmit": true, "skipLibCheck": true, "types": [] }, "include": ["src"] }' > tsconfig.json
printf '%s\n' 'export type Item = { price: number; qty: number };' '' 'export const total = (items: Item[]): number => items.reduce((sum, item) => sum + item.price * item.qty, 0);' > src/total.ts
printf '%s\n' 'import { test, expect } from "vitest";' 'import { total } from "../src/total.ts";' '' 'test("total", () => expect(total([{ price: 2, qty: 3 }])).toBe(6));' > test/total.test.ts
npm install --package-lock-only --ignore-scripts --no-audit --no-fund > /dev/null
git add -A && git commit -qm base && types_base=$(git rev-parse HEAD)
git checkout -q -b broke
printf '%s\n' 'export const version: number = "1.0";' >> src/total.ts
git commit -qam "Name the version" && git rev-parse HEAD > "$W/types-broke.head"
git checkout -q -b fine "$types_base"
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' src/total.ts
git commit -qam "Shorter names" && git rev-parse HEAD > "$W/types-fine.head"
git checkout -q "$types_base"
cd "$W"
check types-broke "$types_base" typesrepo
decision types-broke blocked
[ "$(kinds types-broke types-broke blocked)" = 1 ] && grep -q 'src/total.ts:4 TS2322' types-broke/receipt.md \
  || { cat types-broke/receipt.md; jq .tests.types types-broke/receipt.json; fail "types-broke: a new TypeScript error in a project that type-checked clean was not blocked"; }
jq -e '.tests.status == "RAN" and (.tests.broke | length == 0)' types-broke/receipt.json > /dev/null || { jq .tests types-broke/receipt.json; fail "types-broke: the tests should still pass"; }
check types-fine "$types_base" typesrepo
decision types-fine clear
jq -e '.tests.types.projects[0].result == "same" and .tests.types.projects[0].version == "5.6.3"' types-fine/receipt.json > /dev/null \
  || { jq .tests.types types-fine/receipt.json; fail "types-fine: the type check did not run on both sides"; }

say "A yarn 4 project whose package.json says type module installs: the yarn Kern downloads runs outside the project"
git init -q berryrepo && cd berryrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"berry","version":"1.0.0","private":true,"type":"module","packageManager":"yarn@4.9.2","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' 'nodeLinker: node-modules' > .yarnrc.yml
printf '%s\n' 'export const total = (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0);' > src/total.js
printf '%s\n' 'import { test, expect } from "vitest";' 'import { total } from "../src/total.js";' '' 'test("total", () => expect(total([{ price: 2, qty: 3 }])).toBe(6));' > test/total.test.js
git add -A && git commit -qm base && berry_base=$(git rev-parse HEAD)
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' src/total.js
git commit -qam "Shorter names" && git rev-parse HEAD > "$W/berry.head"
cd "$W"
check berry "$berry_base" berryrepo
jq -e '.tests.status == "RAN" and .tests.kept == 1' berry/receipt.json > /dev/null || { jq .tests berry/receipt.json; jq . berry/setup.json 2>/dev/null | head -40; fail "berry: a yarn 4 project with type module did not install"; }

say "A pnpm 12 project installs with the pnpm it pins through Kern's own corepack, which checks its hash, and its tests can call pnpm offline"
git init -q pnpmrepo && cd pnpmrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"pin","version":"1.0.0","private":true,"type":"module","packageManager":"pnpm@12.14.0+sha512.c18ba6d87993d3d164cb5e8948f32b2c06ee854487e06cd8d86781342fb3e5bb4634367034711e86802baa9c37bea906ce1929ad08d70884c97d801df13389a6","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '%s\n' 'export const total = (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0);' > src/total.js
printf '%s\n' 'import { test, expect } from "vitest";' 'import { execSync } from "node:child_process";' 'import { total } from "../src/total.js";' '' \
  'test("total", () => expect(total([{ price: 2, qty: 3 }])).toBe(6));' 'test("pnpm is there", () => expect(execSync("pnpm --version").toString().trim()).toBe("12.14.0"));' > test/total.test.js
git add -A && git commit -qm base && pnpm_base=$(git rev-parse HEAD)
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' src/total.js
git commit -qam "Shorter names" && git rev-parse HEAD > "$W/pnpm.head"
cd "$W"
check pnpm "$pnpm_base" pnpmrepo
jq -e '.tests.status == "RAN" and .tests.kept == 2' pnpm/receipt.json > /dev/null || { jq .tests pnpm/receipt.json; jq . pnpm/setup.json 2>/dev/null | head -40; fail "pnpm: a pnpm 12 project did not install with its own pnpm, or its tests could not call pnpm"; }
if grep -q 'kern-pnpm-store' pnpm/tests-new.out pnpm/tests-old.out; then fail "pnpm: the test runner found tests inside Kern's own package store"; fi

say "A Bun workspace installs with Kern's own Bun, which npm checks against its sha512, from its bun.lock as Bun installs it (a macOS-only workspace package and a workspace: override, as in eliza; a bunfig.toml that asks for Bun's global store, whose links would point into the cache Kern deletes), and its tests can call bun with the network off"
git init -q bunrepo && cd bunrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p packages/lib packages/app/test packages/mac
printf '%s\n' '{"name":"bunws","private":true,"type":"module","workspaces":["packages/*"],"overrides":{"@x/lib":"workspace:*"},"scripts":{"test":"node --test"}}' > package.json
printf '%s\n' '[install]' 'globalStore = true' > bunfig.toml
printf '%s\n' '{"name":"@x/lib","version":"1.0.0-beta.1","type":"module","exports":"./index.js"}' > packages/lib/package.json
printf '%s\n' 'export const label = (n) => (n === 1 ? "1 item" : n + " items");' > packages/lib/index.js
printf '%s\n' '{"name":"@x/app","version":"1.0.0","private":true,"type":"module","dependencies":{"@x/lib":"workspace:*","is-number":"7.0.0"}}' > packages/app/package.json
printf '%s\n' '{"name":"@x/mac","version":"1.0.0","private":true,"os":["darwin"]}' > packages/mac/package.json
printf '%s\n' "import { test } from 'node:test';" "import assert from 'node:assert/strict';" "import { execFileSync } from 'node:child_process';" "import isNumber from 'is-number';" "import { label } from '@x/lib';" '' \
  "test('labels many', () => assert.equal(label(2), '2 items'));" "test('a registry package loads from node_modules/.bun', () => assert.equal(isNumber(5), true));" \
  "test('bun is there with the network off', () => assert.equal(execFileSync('bun', ['--version'], { encoding: 'utf8' }).trim(), '1.4.2'));" > packages/app/test/app.test.js
cat > bun.lock <<'EOF'
{
  "lockfileVersion": 2,
  "configVersion": 1,
  "workspaces": {
    "": {
      "name": "bunws",
    },
    "packages/app": {
      "name": "@x/app",
      "version": "1.0.0",
      "dependencies": {
        "@x/lib": "workspace:*",
        "is-number": "7.0.0",
      },
    },
    "packages/lib": {
      "name": "@x/lib",
      "version": "1.0.0-beta.1",
    },
    "packages/mac": {
      "name": "@x/mac",
      "version": "1.0.0",
    },
  },
  "overrides": {
    "@x/lib": "workspace:*",
  },
  "packages": {
    "@x/app": ["@x/app@workspace:packages/app"],

    "@x/lib": ["@x/lib@workspace:packages/lib"],

    "@x/mac": ["@x/mac@workspace:packages/mac"],

    "is-number": ["is-number@7.0.0", "", {}, "sha512-41Cifkg6e8TylSpdtTpeLVMqvSBEVzTttHvERD741+pnZ8ANv0004MRL43QKPDlK9cGvNp6NZWZUBlbGXYxxng=="],
  }
}
EOF
git add -A && git commit -qm base && bun_base=$(git rev-parse HEAD)
printf '%s\n' 'export const twice = (n) => n * 2;' >> packages/lib/index.js
git commit -qam "Twice" && git rev-parse HEAD > "$W/bun.head"
cd "$W"
KERN_KEEP=1 check bun "$bun_base" bunrepo
jq -e '[.old.installs[0], .new.installs[0]] | all(.manager == "bun" and .result == "installed")' bun/setup.json > /dev/null \
  || { jq . bun/setup.json 2>/dev/null | head -60; fail "bun: a Bun workspace did not install with Kern's own Bun from its lockfile"; }
jq -e '.tests.status == "RAN" and .tests.kept == 3' bun/receipt.json > /dev/null \
  || { jq .tests bun/receipt.json; fail "bun: the Bun workspace's tests did not run, a registry package's link pointed into Bun's deleted cache, or the tests could not call bun with the network off"; }
"$K" --control verifier.json drive clean bun > /dev/null || fail "bun: kern drive clean could not remove the volumes the kept check left"

say "A project whose .nvmrc asks for Node 20 is installed and tested on Node 20"
git init -q nvmrepo && cd nvmrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"nvm","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}' > package.json
printf '20\n' > .nvmrc
printf '%s\n' 'export const total = (items) => items.reduce((sum, item) => sum + item.price * item.qty, 0);' > src/total.js
printf '%s\n' 'import { test, expect } from "vitest";' 'import { total } from "../src/total.js";' '' \
  'test("runs on the Node the project asks for", () => expect(process.versions.node.split(".")[0]).toBe("20"));' \
  'test("total", () => expect(total([{ price: 2, qty: 3 }])).toBe(6));' > test/total.test.js
git add -A && git commit -qm base && nvm_base=$(git rev-parse HEAD)
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' src/total.js
git commit -qam "Shorter names" && git rev-parse HEAD > "$W/nvm.head"
cd "$W"
check nvm "$nvm_base" nvmrepo
decision nvm clear
jq -e '.tests.status == "RAN" and .tests.kept == 2 and .tests.node.old.major == 20 and .tests.node.new.major == 20' nvm/receipt.json > /dev/null \
  || { jq '.tests | {status, kept, already_failing, node}' nvm/receipt.json; fail "nvm: the project's .nvmrc asked for Node 20 and its tests did not run on Node 20"; }
grep -q 'Node: 20 on both sides, as the project asks' nvm/receipt.md || { cat nvm/receipt.md; fail "nvm: the receipt does not say which Node ran"; }

if [ "${E2E_RUNTIME:-runsc}" = runc ]; then
say "A project whose tests drive a browser gets the browser its Playwright expects, on its own Node"
git init -q browserrepo && cd browserrepo
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src test
printf '%s\n' '{"name":"browser","version":"1.0.0","private":true,"type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4","playwright-core":"1.49.1"}}' > package.json
printf '%s\n' 'export const title = (shop) => `<title>${shop}</title>`;' > src/page.js
printf '%s\n' 'import { test, expect } from "vitest";' 'import { chromium } from "playwright-core";' 'import { title } from "../src/page.js";' '' \
  'test("a real browser reads the page title", async () => {' '  const browser = await chromium.launch();' '  const page = await browser.newPage();' \
  '  await page.setContent(title("shop"));' '  expect(await page.title()).toBe("shop");' '  await browser.close();' '}, 120000);' > test/page.test.js
npm install --package-lock-only --ignore-scripts --no-audit --no-fund > /dev/null
git add -A && git commit -qm base && browser_base=$(git rev-parse HEAD)
sed -i 's/(shop) => `<title>${shop}<\/title>`/(name) => `<title>${name}<\/title>`/' src/page.js
git commit -qam "Clearer name" && git rev-parse HEAD > "$W/browser.head"
cd "$W"
check browser "$browser_base" browserrepo
decision browser clear
jq -e '.tests.status == "RAN" and .tests.kept == 1 and (.tests.already_failing | length == 0)' browser/receipt.json > /dev/null \
  || { jq '.tests | {status, why, kept, already_failing, problem_why, browser}' browser/receipt.json; fail "browser: the test that drives Chromium did not pass on both sides"; }
jq -e '.tests.browser.old.playwright == ["1.49.1"] and .tests.browser.new.playwright == ["1.49.1"] and .tests.browser.new.installed == ["1.49.1"] and (.tests.browser.new.error == null)' browser/receipt.json > /dev/null \
  || { jq '.tests.browser' browser/receipt.json; fail "browser: Chromium was not set up for the playwright-core 1.49.1 the install put on disk"; }
fi

say "Kern check removes its own sandbox volumes"
after=$(volumes)
[ "$after" -le $(( before + 1 )) ] || fail "kern check left $(( after - before )) sandbox volumes behind"

printf '\ne2e-check: PASSED\n'
rm -rf "$W"
