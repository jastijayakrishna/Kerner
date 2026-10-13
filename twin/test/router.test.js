import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire, syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Router, readControl } from '../src/router.js';
import { app, until } from './helpers.js';

const fsp = createRequire(import.meta.url)('node:fs/promises');
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Replaces one function of node:fs/promises, for the router too (its ESM binding follows the change). Answers how to put it back. */
function patch(name, replace) {
  const real = fsp[name];
  fsp[name] = replace(real);
  syncBuiltinESMExports();
  return () => {
    fsp[name] = real;
    syncBuiltinESMExports();
  };
}

/** A router for the refactor app with a state folder. `told` lists what it told the twin's thread: [side, hold] per message. */
async function opened(more = {}) {
  const stateDir = mkdtempSync(join(tmpdir(), 'kern-router-'));
  const told = [];
  const settings = { old: app('refactor', 'old'), candidate: app('refactor', 'new'), candidateId: 'c1', stateDir, onSwitch: (side, reason, hold) => told.push([side, hold]), ...more };
  const router = await Router.open(settings);
  return {
    router, told, settings, file: join(stateDir, 'control.json'),
    done: () => {
      router.close();
      rmSync(stateDir, { recursive: true, force: true });
    },
  };
}

test('a switch back made while the control file is being read is not undone by what that read found', async () => {
  const { router, told, file, done } = await opened();
  let undo = () => {};
  try {
    assert.equal(await router.switchTo('candidate', 'switched on'), true);
    // From here on the router's reads of the control file find what is there, then wait until let go: the switch back comes in between.
    let reads = 0;
    let letGo;
    const gate = new Promise((resolve) => {
      letGo = resolve;
    });
    undo = patch('readFile', (real) => async function (path, ...rest) {
      const text = await real.call(this, path, ...rest);
      if (String(path) === file) {
        reads++;
        await gate;
      }
      return text;
    });
    // A person writes the file, and the candidate stays on. The router looks at it within a second.
    writeFileSync(file, JSON.stringify({ ...JSON.parse(readFileSync(file, 'utf8')), changedAt: new Date().toISOString(), reason: 'a person keeps it on' }));
    await until(() => reads > 0);
    // Meanwhile the twin finds an unexplained difference and switches back.
    const back = router.switchTo('old', 'switched back: an unexplained difference (body) on GET /');
    await Promise.race([back, pause(500)]);
    letGo();
    assert.equal(await back, true);
    await pause(300);
    assert.deepEqual([router.serving, router.hold], ['old', true], 'the switch back holds');
    assert.deepEqual(told, [['candidate', false], ['old', true]], "the twin's thread is never told the candidate serves again");
    const control = await readControl(file);
    assert.deepEqual([control.serving, control.hold], ['old', true]);
  } finally {
    undo();
    done();
  }
});

test("a switch is written to the control file even when telling the twin's thread fails", async () => {
  const { router, file, done } = await opened({ onSwitch: () => {
    throw new Error("the twin's thread is gone");
  } });
  try {
    assert.equal(await router.switchTo('candidate', 'a person switched it on'), true);
    assert.equal(router.serving, 'candidate');
    assert.equal((await readControl(file)).serving, 'candidate', 'serving and the control file agree');
  } finally {
    done();
  }
});

test("the control file's watcher never rejects, even when telling the twin's thread fails", async () => {
  const rejections = [];
  const record = (reason) => rejections.push(String(reason?.message ?? reason));
  process.on('unhandledRejection', record);
  const { router, file, done } = await opened({ onSwitch: () => {
    throw new Error("the twin's thread is gone");
  } });
  try {
    writeFileSync(file, JSON.stringify({ ...JSON.parse(readFileSync(file, 'utf8')), serving: 'candidate', reason: 'a person switched it on' }));
    await until(() => router.serving === 'candidate');
    await pause(200);
    assert.deepEqual(rejections, [], "an unhandled rejection would end the app's process");
  } finally {
    process.off('unhandledRejection', record);
    done();
  }
});
