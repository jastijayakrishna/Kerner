import { performance } from 'node:perf_hooks';

// Sends a scenario's requests to a server, a fixed number at a time, and times each one from the client's side, body included. The
// same seed sends the same requests on every run, at any concurrency: each step gets a random generator of its own.

/**
 * Numbers in [0, 1) from a seed (xorshift32): the same seed always gives the same numbers. The seed is mixed first (murmur3's
 * finalizer): xorshift32 started from neighbouring seeds gives nearly the same first numbers, and each step's seed is its neighbour's
 * plus one (measured: the first number of 300 steps fell in two narrow bands, so a scenario took the same branch every time).
 */
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

/**
 * Runs a scenario against `base`.
 * - `scenario.setup(client)` runs once, untimed, and answers the state the steps share.
 * - `scenario.step(client, state, random, i)` then runs for i from 0 to requests - 1, at most `concurrency` at a time. Every request a
 *   step sends with `client.send(path, init)` is timed.
 * - A step that throws is counted, and the run goes on.
 * Answers { ms, statuses, failed, failures (the first 20), state }.
 */
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
