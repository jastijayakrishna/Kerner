import http from 'node:http';
import { readFile, rm } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

const config = JSON.parse(await readFile(process.env.KERN_PROOF_CONFIG, 'utf8'));
const mode = process.env.KERN_PROOF_MODE === 'off' ? 'off' : 'twin';
const sampleRate = mode === 'off' ? 0 : Number(process.env.KERN_PROOF_SAMPLE ?? 1);
const pgReads = mode === 'twin' && process.env.KERN_PROOF_PG_READS ? process.env.KERN_PROOF_PG_READS : null;
const label = mode === 'off' ? 'off' : `sample-${process.env.KERN_PROOF_SAMPLE ?? '1'}${pgReads ? '-pgreads' : ''}`;
const named = (value) => (typeof value === 'string' ? value.replaceAll('{label}', label) : value);
let twin = null;
let listener;
if (mode === 'off') {
  const app = await import(pathToFileURL(config.old.module).href);
  listener = await app[config.old.export]();
} else {
  const { createTwin } = await import(pathToFileURL(config.twin).href);
  const options = { ...config.options, log: named(config.options.log), cases: named(config.options.cases) };
  if (pgReads) options.pgReads = pgReads;
  for (const path of [options.log, options.cases]) if (path) await rm(path, { recursive: true, force: true });
  twin = await createTwin({ ...options, old: config.old, candidate: config.candidate, sampleRate });
  listener = await twin.listener();
}
let peak = process.memoryUsage().rss;
setInterval(() => {
  peak = Math.max(peak, process.memoryUsage().rss);
}, 250).unref();

const server = http.createServer((req, res) => {
  if (req.url !== '/__proof') return listener(req, res);
  res.writeHead(200, { 'content-type': 'application/json' });
  return res.end(JSON.stringify({
    mode, sampleRate, rss: process.memoryUsage().rss, peakRss: peak,
    stats: twin?.stats() ?? null, decision: twin?.decision() ?? null, evidence: twin?.evidence() ?? null,
    differences: twin?.differences() ?? [], paths: twin?.paths() ?? [],
  }));
});
server.listen(Number(process.env.KERN_PROOF_PORT ?? 8080), '0.0.0.0', () => {
  console.log(`proof server ${config.name} (${mode === 'off' ? 'without the twin' : `sampling ${sampleRate}`}) listening on :${server.address().port}`);
});
