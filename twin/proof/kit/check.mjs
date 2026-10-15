const path = process.argv[2] ?? '/';
const base = 'http://127.0.0.1:8080';
const answer = await fetch(`${base}${path}`);
await answer.arrayBuffer();
console.log(`${path}: ${answer.status}`);
let s = null;
for (let i = 0; i < 180; i++) {
  await new Promise((resolve) => setTimeout(resolve, 1000));
  s = (await (await fetch(`${base}/__proof`)).json()).stats;
  if (!s || s.disabled || (s.queued === 0 && s.same + s.different + s.notCompared + s.skipped >= 1)) break;
}
console.log(JSON.stringify({ disabled: s?.disabled ?? null, same: s?.same, different: s?.different, notCompared: s?.notCompared, skipped: s?.skipped, queued: s?.queued, reasons: s?.reasons }, null, 1));
