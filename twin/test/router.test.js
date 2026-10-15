import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, unwatchFile, writeFileSync } from 'node:fs';
import { createRequire, syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Router, readControl } from '../src/router.js';
import { app, until } from './helpers.js';

const fsp = createRequire(import.meta.url)('node:fs/promises');
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function patch(name, replace) {
  const real = fsp[name];
  fsp[name] = replace(real);
  syncBuiltinESMExports();
  return () => {
    fsp[name] = real;
    syncBuiltinESMExports();
  };
}

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
    writeFileSync(file, JSON.stringify({ ...JSON.parse(readFileSync(file, 'utf8')), changedAt: new Date().toISOString(), reason: 'a person keeps it on' }));
    await until(() => reads > 0);
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

function failWrites(file, failing) {
  return patch('rename', (real) => async function (from, to, ...rest) {
    if (failing.now && String(to) === file) throw Object.assign(new Error('no space left on device'), { code: 'ENOSPC' });
    return real.call(this, from, to, ...rest);
  });
}

test("a switch back whose control file could not be written still holds: the router never takes its own earlier write for a person's switch", async () => {
  const { router, told, file, done } = await opened();
  const undo = [];
  try {
    assert.equal(await router.switchTo('candidate', 'switched on'), true);
    const before = readFileSync(file, 'utf8');
    undo.push(failWrites(file, { now: true }));
    let reads = 0;
    undo.push(patch('readFile', (real) => async function (path, ...rest) {
      if (String(path) === file) reads++;
      return real.call(this, path, ...rest);
    }));
    assert.equal(await router.switchTo('old', 'switched back: an unexplained difference (body) on GET /'), true);
    assert.equal(router.writeErrors, 1);
    assert.equal(readFileSync(file, 'utf8'), before, 'the file still says the candidate serves');
    writeFileSync(file, before);
    const seen = reads;
    await until(() => reads > seen);
    await pause(1200);
    assert.deepEqual([router.serving, router.hold], ['old', true], 'the switch back holds');
    assert.deepEqual(told, [['candidate', false], ['old', true]]);
  } finally {
    for (const back of undo.reverse()) back();
    done();
  }
});

test('a switch back whose control file could not be written is written once it can be, so a restart still serves the old version', async () => {
  const { router, settings, file, done } = await opened();
  const failing = { now: false };
  const undo = failWrites(file, failing);
  try {
    assert.equal(await router.switchTo('candidate', 'switched on'), true);
    failing.now = true;
    assert.equal(await router.switchTo('old', 'switched back: an unexplained difference (body) on GET /'), true);
    assert.equal(router.writeErrors, 1);
    failing.now = false;
    await until(() => JSON.parse(readFileSync(file, 'utf8')).serving === 'old');
    const control = JSON.parse(readFileSync(file, 'utf8'));
    assert.deepEqual([control.serving, control.hold], ['old', true]);
    assert.match(control.reason, /switched back/);
    router.close();
    const again = await Router.open(settings);
    assert.deepEqual([again.serving, again.hold], ['old', true], 'a restart keeps the switch back');
    again.close();
  } finally {
    undo();
    done();
  }
});

test('a write tried again never covers a switch a person wrote to the control file meanwhile', async () => {
  const { router, file, done } = await opened();
  const failing = { now: false };
  const undo = failWrites(file, failing);
  try {
    assert.equal(await router.switchTo('candidate', 'switched on'), true);
    failing.now = true;
    assert.equal(await router.switchTo('old', 'switched back: an unexplained difference (body) on GET /'), true);
    unwatchFile(file);
    const person = JSON.stringify({ ...JSON.parse(readFileSync(file, 'utf8')), serving: 'candidate', hold: false, changedAt: new Date().toISOString(), reason: 'a person checked it' });
    writeFileSync(file, person);
    failing.now = false;
    await pause(2500);
    assert.equal(readFileSync(file, 'utf8'), person, "the person's switch is still there for the router to follow");
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

test('a person who holds the old version in the control file while it already serves is followed: the hold reaches the twin', async () => {
  const { router, told, file, done } = await opened();
  try {
    assert.deepEqual([router.serving, router.hold], ['old', false]);
    writeFileSync(file, JSON.stringify({ ...JSON.parse(readFileSync(file, 'utf8')), hold: true, changedAt: new Date().toISOString(), reason: 'a person holds it' }));
    await until(() => router.hold === true);
    assert.deepEqual(told.at(-1), ['old', true], 'the twin\'s thread is told the hold');
    assert.equal(router.serving, 'old');
  } finally {
    done();
  }
});
