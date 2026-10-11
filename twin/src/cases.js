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

/** Reads one case back, bytes and dates included. */
export async function readCase(file) {
  const value = JSON.parse(await readFile(file, 'utf8'), reviver);
  if (value?.version !== 1 || typeof value.route !== 'string') throw new Error(`${file} is not a Kern twin case (version 1)`);
  return { ...value, file };
}

/** A ghost's answer saved in a case, as compareRuns reads one. */
export function runOf(saved) {
  const body = saved?.body;
  return {
    status: saved?.status, headers: saved?.headers ?? {},
    body: body ? Buffer.from(body.buffer, body.byteOffset, body.byteLength) : Buffer.alloc(0),
    effects: saved?.effects ?? [], events: saved?.events ?? [], error: saved?.error, timedOut: saved?.timedOut,
  };
}
