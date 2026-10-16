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

say "An edited test whose old version fails on the new code needs a look, and one whose old version still passes is clear"
git -C calc checkout -q -b rewrites "$base"
sed -i 's/amount >= 100/amount > 100/' calc/src/total.js
sed -i 's/expect(discount(100)).toBe(90)/expect(discount(100)).toBe(100)/' calc/test/total.test.js
git -C calc commit -qam "Discount above 100, and the test says so" && git -C calc rev-parse HEAD > rewrites.head && git -C calc checkout -q "$base"
check rewrites "$base"
decision rewrites look
[ "$(kinds rewrites test-rewritten look)" = 1 ] && grep -q 'discount from 100' rewrites/receipt.md || { cat rewrites/receipt.md; fail "rewrites: the rewritten expectation was not reported"; }
git -C calc checkout -q -b edits "$base"
sed -i 's/(sum, item) => sum + item.price \* item.qty/(acc, it) => acc + it.price * it.qty/' calc/src/total.js
printf '%s\n' "test('discount of 200', () => expect(discount(200)).toBe(180));" >> calc/test/total.test.js
git -C calc commit -qam "Shorter names, and one more test" && git -C calc rev-parse HEAD > edits.head && git -C calc checkout -q "$base"
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

say "When only the new code's install fails, the receipt quotes the install and needs a look instead of blaming the tests"
git -C calc checkout -q -b installfail "$base"
sed -i 's/"devDependencies":{"vitest":"3.2.4"}/"devDependencies":{"vitest":"3.2.4","@kern-e2e\/does-not-exist":"1.0.0"}/' calc/package.json
git -C calc commit -qam "Add a package that does not exist" && git -C calc rev-parse HEAD > installfail.head && git -C calc checkout -q "$base"
check installfail "$base"
decision installfail look
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

say "Kern plants small faults in the changed lines: one a test notices is caught, and a change no test checks needs a look, with vitest and with jest"
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
  jq -e '.tests.status == "RAN" and ([.tests.faults[] | "\(.path):\(.line) \(.outcome)"] | sort == ["src/price.js:1 caught", "src/price.js:2 missed"])' "faults-some-$runner/receipt.json" > /dev/null \
    || { jq .tests "faults-some-$runner/receipt.json"; fail "faults-some-$runner: the fault a test notices was not caught, or the one no test checks was not missed"; }
  grep -q 'the tests caught 1 of 2 small faults' "faults-some-$runner/receipt.md" && grep -q 'src/price.js:2' "faults-some-$runner/receipt.md" \
    || { cat "faults-some-$runner/receipt.md"; fail "faults-some-$runner: the receipt does not say which planted fault no test noticed"; }
  check "faults-none-$runner" "$faults_base" "faults-$runner-repo"
  decision "faults-none-$runner" look
  jq -e '[.tests.faults[] | "\(.path):\(.line) \(.outcome)"] | sort == ["src/price.js:2 missed", "src/ship.js:1 untested"]' "faults-none-$runner/receipt.json" > /dev/null \
    || { jq .tests "faults-none-$runner/receipt.json"; fail "faults-none-$runner: a change no test checks was not found"; }
  [ "$(kinds "faults-none-$runner" untested-change look)" = 1 ] || { cat "faults-none-$runner/receipt.md"; fail "faults-none-$runner: a change no test checks does not need a look"; }
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

say "Kern check removes its own sandbox volumes"
after=$(volumes)
[ "$after" -le $(( before + 1 )) ] || fail "kern check left $(( after - before )) sandbox volumes behind"

printf '\ne2e-check: PASSED\n'
rm -rf "$W"
