import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { writeCase } from '../src/cases.js';
import { commentBody, forAgent, toFeedback } from '../src/feedback.js';

const cli = fileURLToPath(new URL('../bin/kern-twin.js', import.meta.url));
const base = { version: 1, candidate: 'sha256:abc', segment: 'all', adapters: ['fetch'], overlay: false, lines: {}, scrubbed: true, opaque: [] };
const replayable = (method, url, oldStatus, newStatus) => ({
  world: { start: 1, seed: 1 }, request: { method, url, headers: {}, body: new Uint8Array() }, io: [],
  old: { status: oldStatus, headers: {}, body: new Uint8Array(), effects: [], events: [] }, new: { status: newStatus, headers: {}, body: new Uint8Array(), effects: [], events: [] },
});
const cases = [
  { ...base, n: 1, time: 1, route: 'POST /limit', kinds: ['crash'], pulls: [103], cause: 103, human: false, reproduces: true, ...replayable('POST', '/limit', 200, 500),
    verdict: { kinds: ['crash'], details: [{ kind: 'crash', path: 'result', old: 'status 200', new: "uncaught TypeError: Cannot read properties of null (reading 'max')" }] },
    lines: { 'new:src/app.js:17': '1111' } },
  { ...base, n: 2, time: 2, route: 'GET /admin', kinds: ['door-opened'], pulls: [], cause: null, human: true, reproduces: true, ...replayable('GET', '/admin', 401, 200),
    verdict: { kinds: ['door-opened'], details: [{ kind: 'door-opened', path: 'status', old: '401', new: '200' }] } },
  { ...base, n: 3, time: 3, route: 'POST /email', kinds: ['body'], pulls: [], cause: null, human: false, reproduces: false, why: 'the difference did not show again on the scrubbed copy: it may depend on a private value, so a person must look' },
];
const hidden = (feedback) => 'reason' in feedback || feedback.violations.some((v) => 'allow' in v);

test('cases become kern-feedback/v1 instructions: the agent fixes what it may, and a person is asked about the rest', () => {
  const feedback = toFeedback(cases, { casesDir: '.kern-twin/cases', candidate: 'src/app.js' });
  assert.deepEqual([feedback.schema, feedback.status, feedback.agent_fixable], ['kern-feedback/v1', 'BLOCKED', false]);
  assert.equal(feedback.violations.length, 3);
  const [crash, door, stub] = feedback.violations;
  assert.deepEqual([crash.rule, crash.path], ['twin-difference', 'src/app.js']);
  assert.match(crash.instruction, /kern-twin test --cases \.kern-twin\/cases --candidate src\/app\.js/);
  assert.match(crash.detail, /POST \/limit: crash .*#103/);
  assert.match(door.instruction, /A person must decide/);
  assert.match(stub.instruction, /A person must look/);
  assert.equal(toFeedback([cases[0]], {}).agent_fixable, true);
  assert.equal(crash.allow, `allow: [{ route: 'POST /limit', kinds: ["crash"] }]`);
  const agent = forAgent(feedback);
  assert.ok(!hidden(agent), JSON.stringify(agent));
  assert.deepEqual(agent.violations.map((v) => v.instruction), feedback.violations.map((v) => v.instruction));
  assert.ok(hidden(feedback), 'the full verdict is left as it was');
  const body = commentBody(cases, 'a'.repeat(40), { casesDir: '.kern-twin/cases', candidate: 'src/app.js' });
  assert.match(body, /^### Kern's twin found 3 differences on real requests/);
  assert.match(body, /\| `POST \/limit` \| crash \| #103 \|/);
  assert.match(body, /````json\n\{\n {2}"schema": "kern-feedback\/v1"/);
  assert.doesNotMatch(body, /"allow"|"reason"/, 'the comment is read by the agent: no hints on how to allow a difference');
  assert.match(body, /<!-- kern-twin:v1 head=a{40} -->\n$/);
  assert.doesNotMatch(body, /kern-feedback:v1 head=/, "Kern's own marker is left to Kern");
});

test("a route with a pipe in it stays in its own cell of the comment's table", () => {
  const body = commentBody([{ ...cases[0], route: 'GET /a|b' }], 'a'.repeat(40));
  const row = body.split('\n').find((line) => line.includes('GET /a'));
  assert.equal(row.split(/(?<!\\)\|/).length - 2, 4, row);
  assert.match(row, /`GET \/a\\\|b`/);
});

test('kern-twin feedback and comment read a cases folder', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'kern-feedback-'));
  try {
    for (const value of cases) await writeCase(dir, value);
    const feedback = JSON.parse(execFileSync(process.execPath, [cli, 'feedback', '--cases', dir, '--candidate', 'src/app.js'], { encoding: 'utf8' }));
    assert.equal(feedback.violations.length, 3);
    assert.ok(!hidden(feedback), 'kern-twin feedback prints the copy an agent is handed');
    const body = execFileSync(process.execPath, [cli, 'comment', '--cases', dir, '--candidate', 'src/app.js', '--head', 'b'.repeat(40)], { encoding: 'utf8' });
    assert.match(body, /<!-- kern-twin:v1 head=b{40} -->/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
