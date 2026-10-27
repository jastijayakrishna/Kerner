#!/bin/sh
set -eu
K=${KERN_BIN:?set KERN_BIN to a static Linux kern}
K=$(readlink -f "$K")
W=$(mktemp -d)
say() { printf '\n== %s\n' "$*"; }
fail() { printf '\ne2e-drive: FAILED: %s (everything is kept in %s)\n' "$*" "$W" >&2; exit 1; }
real() { readlink -f "$(command -v "$1")"; }
hash() { sha256sum "$1" | cut -d' ' -f1; }

volumes() { docker volume ls --quiet | grep -c '^kern-deps-' || true; }
before=$(volumes)

say "A throwaway verifier"
cd "$W"
runsc=$(readlink -f "$(docker info --format '{{json .Runtimes.runsc}}' | jq -r .path)")
printf '{"host":"e2e-drive","deployment":"single-vm","reviewed":"%s"}\n' "$(date -u +%F)" > host-policy.json
jq -n --arg docker "$(real docker)" --arg git "$(real git)" --arg runsc "$runsc" --arg version "$("$runsc" --version)" --arg policy "$W/host-policy.json" \
  --arg dh "$(hash "$(real docker)")" --arg gh "$(hash "$(real git)")" --arg rh "$(hash "$runsc")" --arg ph "$(hash host-policy.json)" \
  '{ version: 1, role: "verifier", docker: { file: $docker, sha256: $dh, endpoint: "unix:///var/run/docker.sock" }, git: { file: $git, sha256: $gh },
     runtime: { file: $runsc, sha256: $rh, version: $version, arguments: [] }, host_policy: { file: $policy, sha256: $ph } }' > verifier.json
"$K" --control verifier.json doctor > doctor.json || { cat doctor.json; fail "kern doctor says this machine cannot run the sandbox"; }
docker pull -q node:22-bookworm-slim > /dev/null
image=$(docker image inspect --format '{{.Id}}' node:22-bookworm-slim)
mkdir checks && printf '{"version":1,"cases":[{"command":["/usr/local/bin/npm","test"],"stdout":null}]}\n' > checks/test-suite.json
jq -n --arg image "$image" --arg builder "$(jq -r .builder.sha256 doctor.json)" --argjson check "$("$K" pin-check test-suite checks/test-suite.json)" \
  '{ version: 3, repository: "acme/shop", deployment: "single-vm", executors: ["external"], image: $image,
     builders: { executor: null, verifier: $builder }, checks: [$check], registry: null, permissions: {} }' > operator.json

say "A project and a change to its discount"
git init -q shop && cd shop
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src/lib src/components src/legacy
printf '%s\n' '{"name":"shop","version":"1.0.0","private":true,"type":"module","dependencies":{"react":"19.1.0","react-dom":"19.1.0"}}' > package.json
printf '%s\n' '{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]},"jsx":"preserve","strict":true}}' > tsconfig.json
printf '%s\n' 'export enum Rate { Low = 5, High = 20 }' 'export const rate = (high: boolean): number => (high ? Rate.High : Rate.Low);' > src/lib/tax.ts
printf '%s\n' 'exports.round = function round(x) {' '  return Math.round(x * 100) / 100;' '};' > src/legacy/round.cjs
cat > src/lib/price.ts <<'EOF'
import { rate } from '@/lib/tax';
import { round } from '../legacy/round.cjs';

// The price of `items` items, with tax; ten or more items get a bulk discount.
export function priceFor(items: number, high = false): number {
  const base = items * 10;
  if (items > 10) {
    return round(base * 0.9 * (1 + rate(high) / 100));
  }
  return round(base * (1 + rate(high) / 100));
}
EOF
printf '%s
' 'export function add(a: number, b: number): number {' '  return a + b;' '}' "export default { name: 'store', version: 1 };" > src/lib/store.ts
cat > src/components/Card.tsx <<'EOF'
import { priceFor } from '@/lib/price';

export function Card({ items }: { items: number }) {
  return <div className="card">{items} items cost {priceFor(items)}</div>;
}
EOF
git add -A && git commit -qm base && base=$(git rev-parse HEAD)
sed -i 's/items > 10/items >= 10/; s/0\.9/0.85/' src/lib/price.ts
sed -i 's/className="card">{items} items cost {priceFor(items)}/className="card total">{items} items: {priceFor(items)} EUR/' src/components/Card.tsx
sed -i 's/Math.round(x \* 100) \/ 100/Math.round(x * 100 + Number.EPSILON) \/ 100/' src/legacy/round.cjs
sed -i 's/  return a + b;/  return Math.round(a + b);/' src/lib/store.ts
git commit -qam "Bulk discount from ten items" && head=$(git rev-parse HEAD)
cd "$W"

say "kern drive prepare"
printf '# Bulk discount from ten items\n\nTen or more items get 15%% off; the card shows the total in EUR.\n' > pr.md
"$K" drive prepare "$W/shop" "$base" "$head" "$W/task" pr.md | tee prepare.json
[ "$(jq '.totals.new' prepare.json)" -ge 3 ] && [ "$(jq '.totals.old' prepare.json)" -ge 2 ] || fail "prepare measured too few lines"
grep -q 'items >= 10' task/task/lines.md || fail "lines.md lacks a changed line"

say "A good driver: every changed line runs, both ways"
cat > good.mjs <<'EOF'
const step = (s, f) => { try { console.log(JSON.stringify({ step: s, value: f() })); } catch (e) { console.log(JSON.stringify({ step: s, error: String((e && e.message) || e) })); } };
const price = await import('/workspace/src/lib/price.ts');
for (const n of [1, 10, 11]) step(`priceFor(${n})`, () => price.priceFor(n));
step('priceFor(12, high)', () => price.priceFor(12, true));
const { renderToStaticMarkup } = await import('react-dom/server');
const { createElement } = await import('react');
// A changed module whose default export is no function: Kern's call recorder must hand it on unchanged.
const store = await import('/workspace/src/lib/store.ts');
step('store', () => [store.add(1.4, 2.4), store.default.name]);
const card = await import('/workspace/src/components/Card.tsx');
step('Card(10)', () => renderToStaticMarkup(createElement(card.Card, { items: 10 })));
// A server on any free port: its address must be the same on every run, or nothing it answers could be compared.
const http = await import('node:http');
const server = http.createServer((req, res) => res.end(String(price.priceFor(Number(new URL(req.url, 'http://x').searchParams.get('n'))))));
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const url = `http://127.0.0.1:${server.address().port}/price?n=12`;
const answer = await (await fetch(url)).text();
step('GET /price?n=12', () => ({ url, answer }));
server.close();
EOF
started=$(date +%s)
"$K" --control verifier.json drive run "$W/task" operator.json good.mjs | tee run1.json
echo "first round with setup: $(( $(date +%s) - started )) s"
jq . task/setup.json
[ "$(jq '.coverage.percent' run1.json)" = 100 ] || { jq '.files' task/rounds/1/report.json; cat task/task/feedback.md; fail "the good driver did not run every changed line"; }
[ "$(jq '.steps.different' run1.json)" -ge 3 ] || fail "the changed prices and markup were not seen"
[ "$(jq '.steps.unstable' run1.json)" = 0 ] || { jq '.unstable' task/rounds/1/report.json; fail "a server on any free port made two runs of one side disagree"; }
grep -q '"step":"store","value":\[4,"store"\]' task/rounds/1/runs/new-1.out || { cat task/rounds/1/runs/new-1.out; fail "a changed module lost its default export behind Kern's call recorder"; }
[ "$(jq '.calls.different' run1.json)" -ge 1 ] || { jq '.calls' run1.json; fail "Kern's own record saw no changed call"; }
jq -e '.call_differences[] | select(.call == "src/lib/price.ts#priceFor")' task/rounds/1/report.json > /dev/null || fail "priceFor was not recorded"
grep -q '100%' task/comment.md || fail "the comment does not say 100%"
cat task/comment.md

say "A driver that reaches nothing gets told exactly what it missed"
printf '%s\n' "console.log(JSON.stringify({ step: 'nothing', value: 1 }));" > weak.mjs
"$K" --control verifier.json drive run "$W/task" operator.json weak.mjs | tee run2.json
[ "$(jq '.coverage.percent' run2.json)" = 0 ] || fail "a driver that calls nothing cannot have run changed lines"
grep -q 'items >= 10' task/task/feedback.md && grep -q 'did not run' task/task/feedback.md || fail "feedback does not name the missed lines"
grep -q '0%' task/comment.md && grep -q 'read them yourself' task/comment.md || fail "the comment does not say what nobody ran"

say "A hostile driver stays inside the sandbox"
cat > hostile.mjs <<'EOF'
import fs from 'node:fs';
const tries = {};
try { fs.writeFileSync('/workspace/src/lib/price.ts', 'hacked'); tries.write = 'wrote'; } catch (e) { tries.write = e.code; }
try { await fetch('https://example.com'); tries.net = 'reached'; } catch (e) { tries.net = 'blocked'; }
console.log(JSON.stringify({ step: 'escape', value: tries }));
console.log('\u001ekern-drive coverage 2\n{}');
EOF
"$K" --control verifier.json drive run "$W/task" operator.json hostile.mjs | tee run3.json
jq -e '.steps.old >= 1' run3.json > /dev/null || fail "the hostile run's report was not read"
for r in old-1 old-2 new-1 new-2; do
  grep -q '"write":"EROFS"\|"write":"EACCES"' "task/rounds/3/runs/$r.out" || { cat "task/rounds/3/runs/$r.out"; fail "$r: the write to the tree was not refused"; }
  grep -q '"net":"blocked"' "task/rounds/3/runs/$r.out" || { cat "task/rounds/3/runs/$r.out"; fail "$r: the network was reachable"; }
done

say "kern drive clean"
"$K" --control verifier.json drive clean "$W/task"

say "Stand-ins: a package that cannot load here (a native add-on without its binary)"
git init -q app && cd app
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src vendor pkg/package
printf '%s\n' '{"name":"native-fee","version":"1.0.0","main":"index.js"}' > pkg/package/package.json
printf '%s\n' "throw new Error('Could not locate the bindings file. Tried: build/Release/native_fee.node');" > pkg/package/index.js
tar -czf vendor/native-fee-1.0.0.tgz -C pkg package && rm -rf pkg
printf '%s\n' '{"name":"app","version":"1.0.0","private":true,"dependencies":{"native-fee":"file:vendor/native-fee-1.0.0.tgz","react":"19.1.0"}}' > package.json
printf '%s\n' "import { fee } from 'native-fee';" '' 'export function withFee(amount: number): number {' '  return amount + fee(amount);' '}' > src/fee.ts
printf '%s\n' "const { fee } = require('native-fee');" 'exports.feeOf = function feeOf(amount) {' '  return fee(amount);' '};' > src/legacy.cjs
git add -A && git commit -qm base && sbase=$(git rev-parse HEAD)
sed -i 's/amount + fee(amount)/amount + 2 * fee(amount)/' src/fee.ts
sed -i 's/return fee(amount);/return fee(amount) + 1;/' src/legacy.cjs
git commit -qam "Double the fee" && shead=$(git rev-parse HEAD)
cd "$W"
"$K" drive prepare "$W/app" "$sbase" "$shead" "$W/stand" > /dev/null
mkdir -p drv && cat > drv/driver.mjs <<'EOF'
import { createRequire } from 'node:module';
const step = async (s, f) => { try { console.log(JSON.stringify({ step: s, value: await f() })); } catch (e) { console.log(JSON.stringify({ step: s, error: String((e && e.message) || e) })); } };
await step('withFee(100)', async () => (await import('/workspace/src/fee.ts')).withFee(100));
await step('feeOf(100)', () => createRequire('/workspace/package.json')('./src/legacy.cjs').feeOf(100));
EOF
"$K" --control verifier.json drive run "$W/stand" operator.json drv/driver.mjs > stand1.json
[ "$(jq '.coverage.percent' stand1.json)" = 0 ] || { cat stand/task/feedback.md; fail "code behind a package that throws on load cannot run"; }
grep -q 'stubs/<package>.mjs' stand/task/feedback.md || { cat stand/task/feedback.md; fail "the feedback does not point to stand-ins"; }
mkdir -p drv/stubs && cat > drv/stubs/native-fee.mjs <<'EOF'
import React from 'react';
export const fee = (amount) => amount / 100 + (typeof React.createElement === 'function' ? 0 : 1000);
EOF
"$K" --control verifier.json drive run "$W/stand" operator.json drv/driver.mjs > stand2.json
[ "$(jq '.coverage.percent' stand2.json)" = 100 ] || { cat stand/task/feedback.md; fail "the stand-in did not let the code behind it run"; }
grep -q 'withFee(100)' stand/rounds/2/report.json && grep -q '"old":"{\\"value\\":101}"' stand/rounds/2/report.json && grep -q '"new":"{\\"value\\":102}"' stand/rounds/2/report.json \
  || { jq '.differences, .threw' stand/rounds/2/report.json; fail "the doubled fee was not seen through the stand-in (ES import, and React from the stand-in)"; }
grep -q '"new":"{\\"value\\":2}"' stand/rounds/2/report.json || { jq '.differences' stand/rounds/2/report.json; fail "the CommonJS require did not get the stand-in"; }
grep -q 'stand-ins' stand/comment.md && grep -q 'native-fee' stand/comment.md || { cat stand/comment.md; fail "the comment does not name the stand-in"; }
cat stand/comment.md
"$K" --control verifier.json drive clean "$W/stand" > /dev/null

say "A monorepo: each package's own path aliases (Vite's tsconfig layout too) and a sibling package that was never built"
git init -q mono && cd mono
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p apps/web/src
printf '%s\n' '{"name":"mono","private":true,"workspaces":["apps/*","packages/*"],"dependencies":{"react":"19.1.0","react-dom":"19.1.0"}}' > package.json
mkdir -p packages/money/src apps/site/src
printf '%s\n' '{"name":"@mono/money","version":"1.0.0","main":"dist/index.js","types":"dist/index.d.ts"}' > packages/money/package.json
printf '%s\n' "export * from './cents';" 'export const currency = (code: string): string => code.toUpperCase();' > packages/money/src/index.ts
printf '%s\n' 'export const cents = (n: number): number => Math.round(n * 100);' > packages/money/src/cents.ts
printf '%s\n' '{"name":"site","version":"1.0.0","private":true,"type":"module","dependencies":{"@mono/money":"*"}}' > apps/site/package.json
printf '%s\n' '{ "files": [], "references": [{ "path": "./tsconfig.node.json" }, { "path": "./tsconfig.app.json" }] }' > apps/site/tsconfig.json
printf '%s\n' '{ "compilerOptions": { "baseUrl": ".", "paths": { "~/*": ["./src/*"] } }, "include": ["src"] }' > apps/site/tsconfig.app.json
printf '%s\n' '{ "compilerOptions": { "paths": { "~/*": ["./wrong/*"] } }, "include": ["vite.config.ts"] }' > apps/site/tsconfig.node.json
printf '%s\n' 'export const SPACE = " ";' > apps/site/src/space.ts
printf '%s\n' "import 'server-only';" "import { cents, currency } from '@mono/money';" "import { SPACE } from '~/space.js';" '' \
  'export function label(n: number, code: string): string {' '  return cents(n) + SPACE + currency(code);' '}' > apps/site/src/label.ts
printf '%s\n' '.badge { color: red; }' > apps/web/src/badge.css
printf '%s\n' '.badge { color: blue; }' > apps/web/src/badge.module.css
printf '%s\n' '<svg xmlns="http://www.w3.org/2000/svg"/>' > apps/web/src/logo.svg
printf 'PNG' > apps/web/src/icon.png
cat > apps/web/src/Badge.tsx <<'EOF'
import './badge.css';
import styles from './badge.module.css';
import Logo from './logo.svg';
import icon from './icon.png';

export function Badge({ n }: { n: number }) {
  return <span className={styles.badge} data-icon={icon}><Logo />{n > 9 ? '9+' : n}</span>;
}
EOF
cat > apps/web/src/Reveal.tsx <<'EOF'
import { useEffect, useRef, useState, type ReactNode } from 'react';

export function Reveal({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const seen = new IntersectionObserver(([entry]) => { if (entry.isIntersecting) setShown(true); });
    if (ref.current) seen.observe(ref.current);
    return () => seen.disconnect();
  }, []);
  return <div ref={ref}>{shown ? children : null}</div>;
}
EOF
cat > apps/web/src/Card.tsx <<'EOF'
import { Reveal } from './Reveal';

function Title({ text }: { text: string }) {
  return <h2>{text.toUpperCase()}</h2>;
}

export function Card({ title }: { title: string }) {
  return <Reveal><Title text={title} /></Reveal>;
}
EOF
printf '%s\n' '{"name":"web","version":"1.0.0","private":true,"dependencies":{"@mono/money":"*"}}' > apps/web/package.json
printf '%s\n' "import { cents } from '@mono/money';" '' 'export function price(n: number): number {' '  return cents(n);' '}' > apps/web/src/price.ts
printf '%s\n' '{ // the package'"'"'s own settings' '  "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"], "@lib/*": ["src/*"] }, "jsx": "preserve", },' '}' > apps/web/tsconfig.json
printf '%s\n' "import { VAT } from '@lib/rates';" '' 'export function fee(n: number): number {' '  return (n * VAT) / 1000;' '}' > apps/web/src/fee.ts
printf '%s\n' 'export const VAT = 20;' > apps/web/src/rates.ts
printf '%s\n' "import { VAT } from './rates';" '' 'export default function Hello({ name }) {' '  return <p title={String(VAT)}>Hello {name}</p>;' '}' > apps/web/src/Hello.js
printf '%s\n' 'const Tag = ({ t }) => <em>{t}</em>;' 'module.exports = { Tag };' > apps/web/src/Tag.js
printf '%s\n' "const { Tag } = require('./Tag');" 'module.exports = { tags: (list) => list.map((t) => Tag({ t })) };' > apps/web/src/tags.js
printf '%s\n' "import { VAT } from '@/rates';" '' 'export function total(net: number): number {' '  return net + (net * VAT) / 100;' '}' > apps/web/src/total.ts
printf '%s\n' "import { VAT } from '@/rates';" '' 'export function vatOf(net: number): number {' '  return (net * VAT) / 100;' '}' > apps/web/src/vat.cts
git add -A && git commit -qm base && mbase=$(git rev-parse HEAD)
sed -i 's|return net + (net \* VAT) / 100;|return Math.round(net + (net * VAT) / 100);|' apps/web/src/total.ts
sed -i 's|return (net \* VAT) / 100;|return Math.ceil((net * VAT) / 100);|' apps/web/src/vat.cts
sed -i "s|{n > 9 ? '9+' : n}|{n > 99 ? '99+' : n}|" apps/web/src/Badge.tsx
sed -i 's|return <h2>{text.toUpperCase()}</h2>;|return <h2>{text.trim().toUpperCase()}</h2>;|' apps/web/src/Card.tsx
sed -i 's|return (n \* VAT) / 1000;|return Math.round((n * VAT) / 1000);|' apps/web/src/fee.ts
sed -i 's|  return cents(n);|  return cents(n) + 1;|' apps/web/src/price.ts
sed -i 's|code.toUpperCase()|code.trim().toUpperCase()|' packages/money/src/index.ts
sed -i 's|>Hello {name}<|>Hello, {name}!<|' apps/web/src/Hello.js
sed -i 's|<em>{t}</em>|<strong>{t}</strong>|' apps/web/src/Tag.js
sed -i 's|  return cents(n) + SPACE + currency(code);|  return currency(code) + SPACE + cents(n);|' apps/site/src/label.ts
git commit -qam "Round totals" && mhead=$(git rev-parse HEAD)
cd "$W"
"$K" drive prepare "$W/mono" "$mbase" "$mhead" "$W/monotask" > /dev/null
mkdir -p monodrv/stubs/@lib && printf '%s
' 'export const VAT = 0;' > monodrv/stubs/@lib/rates.mjs
cat > monodrv/mono.mjs <<'EOF'
const step = async (s, f) => { try { console.log(JSON.stringify({ step: s, value: await f() })); } catch (e) { console.log(JSON.stringify({ step: s, error: String((e && e.message) || e) })); } };
await step('total(10.5)', async () => (await import('/workspace/apps/web/src/total.ts')).total(10.5));
await step('vatOf(10.5)', async () => (await import('/workspace/apps/web/src/vat.cts')).vatOf(10.5));
// A stand-in named like the package's own alias (`@lib/rates`) must never replace the project's code.
await step('fee(1001)', async () => (await import('/workspace/apps/web/src/fee.ts')).fee(1001));
// The sibling package was never built: Kern loads its source.
await step('price(1.5)', async () => (await import('/workspace/apps/web/src/price.ts')).price(1.5));
// An ES module (Vite's tsconfig layout, server-only) imports names the sibling's index hands on with export *.
await step('label(2, " eur")', async () => (await import('/workspace/apps/site/src/label.ts')).label(2, ' eur'));
const { renderToStaticMarkup } = await import('react-dom/server');
const { createElement } = await import('react');
// Both outcomes of the changed condition: a line counts as run only when all of it ran.
for (const n of [5, 42, 100]) await step(`Badge(${n})`, async () => renderToStaticMarkup(createElement((await import('/workspace/apps/web/src/Badge.tsx')).Badge, { n })));
await step('Hello', async () => renderToStaticMarkup(createElement((await import('/workspace/apps/web/src/Hello.js')).default, { name: 'kern' })));
await step('tags', async () => renderToStaticMarkup(createElement('div', null, ...(await import('/workspace/apps/web/src/tags.js')).default.tags(['a', 'b']))));
await import('/kern/drive-browser.mjs');
const React = await import('react');
const { createRoot } = await import('react-dom/client');
await step('Card mounted and scrolled into view', async () => {
  const { Card } = await import('/workspace/apps/web/src/Card.tsx');
  const root = createRoot(document.getElementById('root'));
  await React.act(async () => root.render(React.createElement(Card, { title: ' kern ' })));
  await React.act(async () => {});
  return document.getElementById('root').innerHTML;
});
EOF
"$K" --control verifier.json drive run "$W/monotask" operator.json monodrv/mono.mjs > mono.json
[ "$(jq '.coverage.percent' mono.json)" = 100 ] || { cat monotask/task/feedback.md; fail "a package's own path alias, a component's assets or content shown on scroll did not run"; }
[ "$(jq '.steps.different' mono.json)" = 10 ] || { jq '.differences' monotask/rounds/1/report.json; fail "the rounding, the badges, the card title, the fee, the price, the label, the greeting and the tags were not seen"; }
jq -e '.call_differences[] | select(.call == "apps/web/src/Hello.js#default")' monotask/rounds/1/report.json > /dev/null || fail "the calls into a CommonJS module's default export were not recorded"
jq -e '.call_differences[] | select(.call == "apps/web/src/Tag.js#Tag")' monotask/rounds/1/report.json > /dev/null || fail "the calls into a required CommonJS module with JSX were not recorded"
grep -q '"step":"fee(1001)","value":20}' monotask/rounds/1/runs/new-1.out || { cat monotask/rounds/1/runs/new-1.out; fail "a stand-in replaced the project's own module behind its alias"; }
grep -q 'class=\\"badge\\"' monotask/rounds/1/runs/new-1.out && grep -q 'data-icon=\\"/assets/icon.png\\"' monotask/rounds/1/runs/new-1.out \
  || { cat monotask/rounds/1/runs/new-1.out; fail "the badge's assets did not load as a bundler gives them"; }
"$K" --control verifier.json drive clean "$W/monotask" > /dev/null

say "Setup runs no code from the repository: a .pnpmfile.cjs never runs"
git init -q pnpmapp && cd pnpmapp
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p src
printf '%s\n' '{"name":"pnpmapp","version":"1.0.0","private":true,"packageManager":"pnpm@9.15.9"}' > package.json
printf '%s\n' "lockfileVersion: '9.0'" '' 'settings:' '  autoInstallPeers: true' '  excludeLinksFromLockfile: false' '' 'importers:' '' '  .: {}' > pnpm-lock.yaml
printf '%s\n' "require('fs').writeFileSync(__dirname + '/PNPMFILE-RAN', 'yes');" 'module.exports = { hooks: {} };' > .pnpmfile.cjs
printf '%s\n' 'export const x = () => 1;' > src/x.js
git add -A && git commit -qm base && pbase=$(git rev-parse HEAD)
sed -i 's/=> 1;/=> 2;/' src/x.js && git commit -qam two && phead=$(git rev-parse HEAD)
cd "$W"
"$K" drive prepare "$W/pnpmapp" "$pbase" "$phead" "$W/pnpmtask" > /dev/null
mkdir -p pnpmdrv && cat > pnpmdrv/driver.mjs <<'EOF'
import fs from 'node:fs';
console.log(JSON.stringify({ step: 'x()', value: (await import('/workspace/src/x.js')).x() }));
console.log(JSON.stringify({ step: 'pnpmfile ran', value: fs.existsSync('/workspace/PNPMFILE-RAN') }));
EOF
"$K" --control verifier.json drive run "$W/pnpmtask" operator.json pnpmdrv/driver.mjs > pnpm.json
jq -e '.coverage.percent == 100' pnpm.json > /dev/null || { cat pnpmtask/task/feedback.md; jq '.installs' pnpmtask/rounds/1/report.json; fail "the pnpm project did not run"; }
grep -q '"step":"pnpmfile ran","value":false' pnpmtask/rounds/1/runs/new-1.out || { cat pnpmtask/rounds/1/runs/new-1.out; fail "the repository's .pnpmfile.cjs ran during setup"; }
"$K" --control verifier.json drive clean "$W/pnpmtask" > /dev/null

say "A Bun project installs with Kern's own Bun, its workspace: and catalog: versions as Bun reads them; a lockfile that does not match its manifests is said"
git init -q bunapp && cd bunapp
git config user.email e2e@example.invalid && git config user.name e2e
mkdir -p packages/lib/src packages/app/src
printf '%s\n' '{"name":"bunapp","private":true,"packageManager":"bun@1.3.13","workspaces":{"packages":["packages/*"],"catalog":{"is-number":"7.0.0"}}}' > package.json
printf '%s\n' '{"lockfileVersion": 1, "workspaces": {}}' > bun.lock
printf '%s\n' '{"name":"@bun/lib","version":"1.0.0","main":"src/index.ts"}' > packages/lib/package.json
printf '%s\n' 'export const twice = (n: number): number => n * 2;' > packages/lib/src/index.ts
printf '%s\n' '{"name":"@bun/app","version":"1.0.0","dependencies":{"@bun/lib":"workspace:*","is-number":"catalog:"}}' > packages/app/package.json
printf '%s\n' "import { twice } from '@bun/lib';" "import isNumber from 'is-number';" '' 'export function run(n: unknown): number {' '  return isNumber(n) ? twice(Number(n)) : 0;' '}' > packages/app/src/run.ts
git add -A && git commit -qm base && bbase=$(git rev-parse HEAD)
sed -i 's|twice(Number(n)) : 0;|twice(Number(n)) + 1 : 0;|' packages/app/src/run.ts && git commit -qam plus && bhead=$(git rev-parse HEAD)
cd "$W"
"$K" drive prepare "$W/bunapp" "$bbase" "$bhead" "$W/buntask" > /dev/null
mkdir -p bundrv && cat > bundrv/driver.mjs <<'EOF'
import fs from 'node:fs';
const { run } = await import('/workspace/packages/app/src/run.ts');
console.log(JSON.stringify({ step: 'run(2)', value: run(2) }));
console.log(JSON.stringify({ step: "run('x')", value: run('x') }));
// The project's own package.json comes back as it was.
console.log(JSON.stringify({ step: 'lib version', value: JSON.parse(fs.readFileSync('/workspace/packages/app/package.json', 'utf8')).dependencies['@bun/lib'] }));
EOF
"$K" --control verifier.json drive run "$W/buntask" operator.json bundrv/driver.mjs > bun.json
jq -e '.coverage.percent == 100' bun.json > /dev/null || { cat buntask/task/feedback.md; jq '.installs' buntask/rounds/1/report.json; fail "the Bun project did not run"; }
grep -q '"step":"run(2)","value":5}' buntask/rounds/1/runs/new-1.out && grep -q '"step":"lib version","value":"workspace:\*"}' buntask/rounds/1/runs/new-1.out \
  || { cat buntask/rounds/1/runs/new-1.out; fail "the Bun workspace did not link, or its package.json did not come back"; }
grep -q "the lockfile is out of date" buntask/comment.md || { cat buntask/comment.md; fail "the comment did not say the Bun lockfile was out of date"; }
"$K" --control verifier.json drive clean "$W/buntask" > /dev/null

say "Every cleaned task leaves no volume behind (only Kern's tools stay, once, in the dependency cache)"
after=$(volumes)
[ "$after" -le $((before + 1)) ] || fail "Kern left $((after - before)) volumes behind"
printf '\ne2e-drive: PASSED (%s)\n' "$W"
