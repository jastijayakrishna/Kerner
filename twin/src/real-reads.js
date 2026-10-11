import { readCapped } from './record.js';
import { exactBytes } from './serialize.js';
import { readAllowed } from './keys.js';

const HOP = new Set(['connection', 'keep-alive', 'transfer-encoding', 'content-length', 'host', 'proxy-connection', 'te', 'trailer', 'upgrade']);

/**
 * One allow-listed real read, made by the twin's thread for both ghosts of a request. It must be a GET or HEAD to an address `realReads`
 * lists; it gets at most 5 s, and its answer is capped at maxBodyBytes. The answer comes back decoded, so it carries no content-encoding.
 * Answers { status, headers, body } or { error }.
 */
export async function realRead({ method, url, headers }, patterns, maxBodyBytes) {
  if (!readAllowed(patterns, method, url)) return { error: 'not an allowed real read' };
  try {
    const sent = Object.fromEntries(Object.entries(headers ?? {}).filter(([name]) => !HOP.has(name.toLowerCase())));
    const response = await fetch(url, { method, headers: sent, redirect: 'manual', signal: AbortSignal.timeout(5000) });
    const body = method === 'HEAD' ? Buffer.alloc(0) : await readCapped(response.body, maxBodyBytes);
    if (body === null) return { error: 'the answer was over the limit' };
    const answer = Object.fromEntries([...response.headers].filter(([name]) => name !== 'content-encoding' && name !== 'content-length'));
    return { status: response.status, headers: answer, body: exactBytes([body]) };
  } catch (error) {
    return { error: String(error?.message ?? error) };
  }
}
