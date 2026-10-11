import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

// The manifest `kern twin plan` writes: every changed line of code of a change, with the commit and pull request behind it. Line keys
// are '<side>:<path>:<line>', for example 'old:src/app.js:17'.
const SIDES = ['old', 'new'];

/** A short hash of a line's text: the evidence keeps it beside each line's counts, so they can be matched to the text they belong to. */
export const lineHash = (text) => createHash('sha256').update(String(text)).digest('hex').slice(0, 16);

/** Reads a manifest and checks its shape. Answers it with `lines`: a Map from each line key to { side, path, line, text, hash, commit, pull }. */
export async function loadManifest(file) {
  let manifest;
  try {
    manifest = JSON.parse(await readFile(file, 'utf8'));
  } catch (error) {
    throw new Error(`the manifest ${file} is not JSON: ${error.message}`);
  }
  return checkManifest(manifest);
}

export function checkManifest(manifest) {
  if (manifest?.version !== 1 || !Array.isArray(manifest.files) || !manifest.commits || typeof manifest.commits !== 'object') throw new Error('not a kern twin plan manifest (version 1)');
  const lines = new Map();
  for (const file of manifest.files) {
    const path = file?.path;
    if (typeof path !== 'string' || !path || path.startsWith('/') || /^[A-Za-z]:/.test(path) || path.split('/').includes('..')) throw new Error(`a manifest path must stay inside the repository: ${path}`);
    for (const side of SIDES) {
      for (const [number, entry] of Object.entries(file[side] ?? {})) {
        const line = Number(number);
        if (!Number.isInteger(line) || line < 1 || typeof entry?.text !== 'string') throw new Error(`a manifest line must be { text, commit }: ${side} ${path}:${number}`);
        const commit = typeof entry.commit === 'string' ? entry.commit : null;
        const pull = commit && Number.isInteger(manifest.commits[commit]?.pull) ? manifest.commits[commit].pull : null;
        lines.set(`${side}:${path}:${line}`, { side, path, line, text: entry.text, hash: lineHash(entry.text), commit, pull });
      }
    }
  }
  return { ...manifest, lines };
}

/** What the ghost of one side measures: { side, root, files: [{ path, lines }] }. */
export function coveragePlan(manifest, side, root) {
  const files = [];
  for (const file of manifest.files) {
    const lines = Object.keys(file[side] ?? {}).map(Number);
    if (lines.length) files.push({ path: file.path, lines });
  }
  return { side, root, files };
}

/**
 * Checks that the code under each root ({ old, new }) is the code the manifest describes: every changed line's text is at its line.
 * kern clips lines over 200 characters and ends them with '...'. Throws, naming the first line that is not there.
 */
export async function checkRoots(manifest, roots) {
  for (const file of manifest.files) {
    for (const side of SIDES) {
      const entries = Object.entries(file[side] ?? {});
      if (!entries.length) continue;
      let text;
      try {
        text = await readFile(join(roots[side], file.path), 'utf8');
      } catch {
        throw new Error(`the ${side} code has no file ${file.path} under ${roots[side]}`);
      }
      const lines = text.split('\n').map((line) => line.replace(/\r$/, ''));
      for (const [number, entry] of entries) {
        const found = lines[Number(number) - 1] ?? '';
        const same = entry.text.endsWith('...') ? found.startsWith(entry.text.slice(0, -3)) : found === entry.text;
        if (!same) throw new Error(`the ${side} code at ${roots[side]} is not what the manifest describes: ${file.path}:${number}`);
      }
    }
  }
}

/**
 * The pull requests whose changed lines a request ran (keys of either side). `cause` is that pull request when exactly one ran and every
 * line run has a known one; otherwise null.
 */
export function attribute(keys, manifest) {
  const pulls = new Set();
  let unknown = false;
  for (const key of keys) {
    const line = manifest.lines.get(key);
    if (!line) continue;
    if (line.pull === null) unknown = true;
    else pulls.add(line.pull);
  }
  const list = [...pulls].sort((a, b) => a - b);
  return { pulls: list, cause: list.length === 1 && !unknown ? list[0] : null };
}

/** A test for `accept` entries, which look like 'old:src/x.js:12' or 'new:src/x.js:20-24'. Throws a TypeError on another shape. */
export function acceptMatcher(patterns) {
  const rules = patterns.map((pattern) => {
    const m = /^(old|new):(.+):(\d+)(?:-(\d+))?$/.exec(String(pattern));
    if (!m) throw new TypeError(`accept entries look like 'old:src/x.js:12' or 'new:src/x.js:20-24', not '${pattern}'`);
    return { side: m[1], path: m[2], from: Number(m[3]), to: Number(m[4] ?? m[3]) };
  });
  return (key) => {
    const m = /^(old|new):(.+):(\d+)$/.exec(key);
    if (!m) return false;
    const line = Number(m[3]);
    return rules.some((rule) => rule.side === m[1] && rule.path === m[2] && line >= rule.from && line <= rule.to);
  };
}
