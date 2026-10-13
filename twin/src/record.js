import { current } from './capture.js';
import { callKey, isWrite } from './keys.js';
import { exactBytes } from './serialize.js';

let installed = false;

/** Reads a stream to its end; null when it is larger than `cap` bytes (the rest is cancelled). */
export async function readCapped(stream, cap) {
  if (!stream) return Buffer.alloc(0);
  const reader = stream.getReader();
  const parts = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return Buffer.concat(parts);
    size += value.byteLength;
    if (size > cap) {
      await reader.cancel().catch(() => {});
      return null;
    }
    parts.push(Buffer.from(value));
  }
}

/** Writes down every fetch a sampled request makes: what was asked and what came back. Calls outside sampled requests pass straight through. */
export function recordFetch(maxBodyBytes) {
  if (installed) return;
  installed = true;
  const realFetch = globalThis.fetch;
  globalThis.fetch = function kernFetch(input, init) {
    const record = current.getStore();
    if (!record || record.done) return realFetch(input, init);
    return recorded(record, realFetch, input, init, maxBodyBytes);
  };
}

async function recorded(record, realFetch, input, init, maxBodyBytes) {
  // The copy is complete only once the answer has been read to the end; finish() waits for this, a little.
  let settle;
  const tracked = new Promise((resolve) => {
    settle = resolve;
  });
  record.pending.add(tracked);
  tracked.then(() => record.pending.delete(tracked));
  let request;
  try {
    request = new Request(input, init);
  } catch {
    settle();
    return realFetch(input, init);
  }
  let body;
  try {
    body = request.body ? await readCapped(request.clone().body, maxBodyBytes) : Buffer.alloc(0);
  } catch {
    body = null;
  }
  if (body === null) {
    record.problem ??= 'an outgoing call had a body Kern could not copy';
    settle();
    return realFetch(request);
  }
  const method = request.method.toUpperCase();
  // `sent` is what the app sent: Phase 5 scrubs it and names the call again from the scrubbed copy.
  const call = { kind: 'fetch', key: callKey(method, request.url, body), method, url: request.url, write: isWrite(method), sent: exactBytes([body]), status: 0, headers: {}, body: null, error: null };
  record.io.push(call);
  let response;
  try {
    response = await realFetch(request);
  } catch (error) {
    // A call that failed for real fails the same way in both ghosts.
    call.error = String(error?.message ?? error);
    settle();
    throw error;
  }
  call.status = response.status;
  call.headers = Object.fromEntries(response.headers);
  // Each cookie an answer sets is a Set-Cookie header of its own, and Object.fromEntries keeps only the last: they are kept as a list,
  // as node:http gives them.
  const cookies = response.headers.getSetCookie();
  if (cookies.length) call.headers['set-cookie'] = cookies;
  readCapped(response.clone().body, maxBodyBytes).then(
    (copy) => {
      if (copy === null) record.problem ??= 'an outgoing call answered with a body over the limit';
      else call.body = exactBytes([copy]);
    },
    () => {
      record.problem ??= 'an outgoing call\'s answer could not be copied';
    },
  ).finally(settle);
  return response;
}
