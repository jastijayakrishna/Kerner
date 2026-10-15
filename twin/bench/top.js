import { readFileSync } from 'node:fs';

const profile = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const nodes = new Map(profile.nodes.map((node) => [node.id, node]));
const own = new Map();
let total = 0;
profile.samples.forEach((id, i) => {
  const { functionName, url, lineNumber } = nodes.get(id).callFrame;
  const key = `${functionName || '(anonymous)'}  ${url ? `${url.split('/').slice(-2).join('/')}:${lineNumber + 1}` : '(native)'}`;
  const us = profile.timeDeltas[i] ?? 0;
  own.set(key, (own.get(key) ?? 0) + us);
  total += us;
});
for (const [key, us] of [...own].sort((a, b) => b[1] - a[1]).slice(0, 15)) {
  console.log(`${(us / 1000).toFixed(1).padStart(9)} ms ${((us / total) * 100).toFixed(1).padStart(5)}%  ${key}`);
}
