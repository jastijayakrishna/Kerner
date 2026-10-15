const base = process.env.SHOP_URL ?? 'http://app:3000';
let seed = 7;
const random = () => {
  seed = (seed * 1103515245 + 12345) % 2147483648;
  return seed / 2147483648;
};
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

for (let i = 0; ; i++) {
  try {
    await (await fetch(`${base}/__twin`)).json();
    break;
  } catch {
    if (i > 120) throw new Error('the shop did not start');
    await pause(500);
  }
}

const orders = [];
for (let i = 0; i < 200; i++) {
  const pick = random();
  if (pick < 0.4) await (await fetch(`${base}/products`)).arrayBuffer();
  else if (pick < 0.7) await (await fetch(`${base}/products/${1 + Math.floor(random() * 12)}`)).arrayBuffer();
  else if (pick < 0.95) {
    const quantity = (1 + Math.floor(random() * 3)) * (random() < 0.15 ? 6 : 1);
    const answer = await fetch(`${base}/orders`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ productId: 1 + Math.floor(random() * 10), quantity }) });
    const value = await answer.json();
    if (answer.status === 201) orders.push(value.id);
  } else if (orders.length) await (await fetch(`${base}/orders/${orders[Math.floor(random() * orders.length)]}`)).arrayBuffer();
}

let report;
for (let i = 0; i < 240; i++) {
  report = await (await fetch(`${base}/__twin`)).json();
  if (report.stats.queued === 0) break;
  await pause(500);
}
const s = report.stats;
const judged = s.same + s.stated + s.different + s.notCompared + s.skipped;
const compared = judged ? (s.same + s.stated + s.different) / judged : 0;
console.log(JSON.stringify({ ...s, compared: Number(compared.toFixed(4)), differences: report.differences.map((d) => ({ route: d.route, kinds: d.kinds, details: d.details.slice(0, 2) })) }, null, 2));
if (compared < 0.95) {
  console.log(`FAILED: the twin compared ${(compared * 100).toFixed(1)}% of ${judged} sampled requests, under 95%`);
  process.exit(1);
}
console.log(`PASSED: the twin compared ${(compared * 100).toFixed(1)}% of ${judged} sampled requests`);
