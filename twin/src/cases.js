import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

// A case: one unexplained difference, kept so the change can be fixed and the fix proven. It holds:
// - the scrubbed request, with its recording;
// - the world it ran in (clock and seed);
// - the old and the new ghost's answers on that scrubbed copy;
// - what differs, the changed lines it ran and their pull requests.
// Bytes are written as { "$bytes": "<base64>" } and dates as { "$date": "<ISO>" }, so a case reads back exactly.

function replacer(key, value) {
  const raw = this[key];
  if (raw instanceof Uint8Array) return { $bytes: Buffer.from(raw.buffer, raw.byteOffset, raw.byteLength).toString('base64') };
  if (raw instanceof Date) return { $date: raw.toISOString() };
  return value;
}

function reviver(key, value) {
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    const names = Object.keys(value);
    if (names.length === 1 && typeof value.$bytes === 'string') return new Uint8Array(Buffer.from(value.$bytes, 'base64'));
    if (names.length === 1 && typeof value.$date === 'string') return new Date(value.$date);
  }
  return value;
}

const slug = (route) => String(route).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60) || 'request';

/** A case's file name: when its request came, its number, and its route. */
export const caseName = (entry) => `${entry.time}-${entry.n}-${slug(entry.route)}.json`;

/** Writes one case whole (a temporary file, then a rename) and answers its path. */
export async function writeCase(dir, entry) {
  await mkdir(dir, { recursive: true });
  const file = join(dir, caseName(entry));
  const temp = `${file}.${process.pid}.tmp`;
  try {
    await writeFile(temp, `${JSON.stringify(entry, replacer, 2)}\n`);
    await rename(temp, file);
  } catch (error) {
    await rm(temp, { force: true }).catch(() => {});
    throw error;
  }
  return file;
}

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

/** What a case that reproduces lacks to run again on both ghosts, or null when it holds it all. */
function lacking(value) {
  const { world, request } = value;
  if (!isObject(world) || !Number.isFinite(world.start) || !Number.isFinite(world.seed)) return 'its world needs a start and a seed (numbers)';
  if (!isObject(request) || typeof request.method !== 'string' || typeof request.url !== 'string' || !isObject(request.headers) || !(request.body instanceof Uint8Array)) {
    return 'its request needs a method, a url, headers and body bytes';
  }
  // Each recorded call is named by its kind and key, which the ghosts look it up by.
  if (!Array.isArray(value.io) || !value.io.every((call) => isObject(call) && typeof call.kind === 'string' && typeof call.key === 'string')) {
    return 'its io must be a list of calls, each with a kind and a key';
  }
  if (!isObject(value.old)) return 'it has no old answer';
  if (!isObject(value.new)) return 'it has no new answer';
  return null;
}

/**
 * Reads one case back, bytes and dates included. A file that is not a case, or a case that says it reproduces but lacks what running
 * it again takes, cannot be read: the twin then lists it as unreadable, which holds the candidate, instead of failing inside.
 */
export async function readCase(file) {
  const value = JSON.parse(await readFile(file, 'utf8'), reviver);
  if (value?.version !== 1 || typeof value.route !== 'string') throw new Error(`${file} is not a Kern twin case (version 1)`);
  // Every case names its kinds: the feedback, the comment and the known-case check all read them.
  if (!Array.isArray(value.kinds) || !value.kinds.every((kind) => typeof kind === 'string')) throw new Error(`${file} is a broken Kern twin case: its kinds must be a list of names`);
  const lack = value.reproduces ? lacking(value) : null;
  if (lack) throw new Error(`${file} is a broken Kern twin case: ${lack}`);
  return { ...value, file };
}

/** A ghost's answer saved in a case, as compareRuns reads one. */
export function runOf(saved) {
  const body = saved?.body;
  return {
    status: saved?.status, headers: saved?.headers ?? {},
    body: body ? Buffer.from(body.buffer, body.byteOffset, body.byteLength) : Buffer.alloc(0),
    effects: saved?.effects ?? [], writes: saved?.writes, tooMany: saved?.tooMany, events: saved?.events ?? [], error: saved?.error, timedOut: saved?.timedOut,
  };
}
