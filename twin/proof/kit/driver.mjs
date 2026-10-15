import { performance } from 'node:perf_hooks';

export function seeded(seed) {
  let state = seed >>> 0;
  state = Math.imul(state ^ (state >>> 16), 0x85ebca6b) >>> 0;
  state = Math.imul(state ^ (state >>> 13), 0xc2b2ae35) >>> 0;
  state = (state ^ (state >>> 16)) >>> 0 || 0x9e3779b9;
  return () => {
    state ^= state << 13;
    state >>>= 0;
    state ^= state >>> 17;
    state ^= state << 5;
    state >>>= 0;
    return state / 4294967296;
  };
}

export async function drive({ base, scenario, requests, concurrency = 1, seed = 7 }) {
  const ms = [];
  const statuses = {};
  const failures = [];
  let failed = 0;
  let timing = false;
  const client = {
    async send(path, init = {}) {
      const started = performance.now();
      const answer = await fetch(`${base}${path}`, init);
      const bytes = Buffer.from(await answer.arrayBuffer());
      if (timing) {
        ms.push(performance.now() - started);
        statuses[answer.status] = (statuses[answer.status] ?? 0) + 1;
      }
      return { status: answer.status, headers: answer.headers, bytes, json: () => JSON.parse(bytes.toString('utf8')) };
    },
  };
  const state = scenario.setup ? await scenario.setup(client) : {};
  timing = true;
  let next = 0;
  const worker = async () => {
    while (next < requests) {
      const i = next++;
      try {
        await scenario.step(client, state, seeded(seed * 1000003 + i), i);
      } catch (error) {
        failed++;
        if (failures.length < 20) failures.push(`step ${i}: ${error?.message ?? error}`);
      }
    }
  };
  await Promise.all(Array.from({ length: concurrency }, worker));
  return { ms, statuses, failed, failures, state };
}
