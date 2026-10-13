// The cases a twin wrote, as node:test tests. Each one runs the candidate on one case in a fresh sealed ghost, and expects the old
// version's answer. kern-twin test runs this file, with the cases folder in KERN_TWIN_CASES and the candidate's module and export in
// KERN_TWIN_CANDIDATE and KERN_TWIN_EXPORT.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync } from 'node:fs';
import { join } from 'node:path';
import { readCase, runOf } from './cases.js';
import { replayCase } from './replay.js';
import { compareRuns } from './compare.js';

const dir = process.env.KERN_TWIN_CASES;
const candidate = { module: process.env.KERN_TWIN_CANDIDATE, export: process.env.KERN_TWIN_EXPORT || 'createApp' };

for (const name of readdirSync(dir).filter((file) => file.endsWith('.json')).sort()) {
  let value;
  try {
    value = await readCase(join(dir, name));
  } catch (error) {
    // A case that cannot be read fails as a test of its own, by name: it never passes quietly, and every other case still runs.
    test(`${name}: could not be read`, () => {
      throw error;
    });
    continue;
  }
  if (!value.reproduces) {
    test(`${name}: ${value.route}`, { skip: value.why }, () => {});
    continue;
  }
  test(`${name}: ${value.route} answers as the old version did`, async () => {
    const verdict = compareRuns(runOf(value.old), await replayCase(value, candidate));
    assert.equal(verdict.outcome, 'same', `${verdict.kinds.join(', ')}: ${JSON.stringify(verdict.details.slice(0, 3))}`);
  });
}
