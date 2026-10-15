export const DEFAULT_HUMANS = { kinds: ['door-opened'], hosts: [] };

export function needsHuman(verdict, humans = {}) {
  const kinds = new Set([...DEFAULT_HUMANS.kinds, ...(humans.kinds ?? [])]);
  if (verdict.kinds.some((kind) => kinds.has(kind))) return true;
  const hosts = humans.hosts ?? [];
  return verdict.details.some((d) => d.kind === 'effects' && hosts.some((host) => String(d.old).includes(host) || String(d.new).includes(host)));
}

export const SCHEMA = 'kern-feedback/v1';
const MARKER = '<!-- kern-twin:v1 head=';

const code = (text) => `\`${String(text).replace(/`/g, "'").replace(/[\r\n]+/g, ' ').slice(0, 200)}\``;
const cell = (text) => text.replace(/\|/g, '\\|');

function pathOf(value) {
  const key = Object.keys(value.lines ?? {}).find((k) => k.startsWith('new:')) ?? Object.keys(value.lines ?? {})[0];
  return key ? key.split(':').slice(1, -1).join(':') : null;
}

function summary(value) {
  const first = value.verdict?.details?.[0];
  const what = first ? `${value.kinds.join(', ')} at ${first.path}: ${first.old} → ${first.new}` : value.kinds.join(', ');
  const pull = value.cause ? `, from #${value.cause}` : value.pulls?.length ? `, from one of ${value.pulls.map((p) => `#${p}`).join(', ')}` : '';
  return `${value.route}: ${what}${pull}`.slice(0, 400);
}

function instructionFor(value, { casesDir = '<cases folder>', candidate = '<entry module>' }) {
  if (!value.reproduces) return `A person must look: ${value.why}`;
  if (value.human) return `A person must decide whether this change is meant (${value.kinds.join(', ')}); an agent does not decide this alone.`;
  return `Run \`npx kern-twin test --cases ${casesDir} --candidate ${candidate}\`: case ${value.file ? value.file.split(/[\\/]/).pop() : value.n} must answer ${value.route} as the old version did. Change the new code until it does. If the difference is meant, say so in the pull request instead, and a person adds an allow rule.`;
}

export function toFeedback(cases, options = {}) {
  const violations = cases.map((value) => {
    const violation = { rule: 'twin-difference' };
    const path = pathOf(value);
    if (path) violation.path = path;
    violation.detail = summary(value);
    violation.instruction = instructionFor(value, options);
    violation.allow = `allow: [{ route: '${value.route}', kinds: ${JSON.stringify(value.kinds)} }]`;
    return violation;
  });
  return {
    schema: SCHEMA,
    status: 'BLOCKED',
    reason: `the twin found ${cases.length} unexplained difference${cases.length === 1 ? '' : 's'} on real requests`,
    agent_fixable: cases.length > 0 && cases.every((value) => value.reproduces && !value.human),
    violations,
  };
}

export function forAgent(feedback) {
  const { reason, ...rest } = feedback;
  return { ...rest, violations: feedback.violations.map(({ allow, ...violation }) => violation) };
}

export function commentBody(cases, head, options = {}) {
  const feedback = toFeedback(cases, options);
  const rows = cases.map((value) => `| ${cell(code(value.route))} | ${value.kinds.join(', ')} | ${value.cause ? `#${value.cause}` : value.pulls?.length ? value.pulls.map((p) => `#${p}`).join(', ') : 'unknown'} | ${!value.reproduces ? 'a person must look' : value.human ? 'a person must decide' : 'the agent can fix it'} |`);
  const lead = feedback.agent_fixable
    ? `To fix: run ${code(`npx kern-twin test --cases ${options.casesDir ?? '<cases folder>'} --candidate ${options.candidate ?? '<entry module>'}`)} and change the new code until every case answers as the old version did. If a difference is meant, say so in the pull request.`
    : 'Some of these need a person before an agent changes anything; the table says which.';
  const machine = JSON.stringify(forAgent(feedback), null, 2).replace(/`/g, '\\u0060');
  const hex = String(head).replace(/[^0-9a-f]/gi, '').slice(0, 64);
  return [
    `### Kern's twin found ${cases.length} difference${cases.length === 1 ? '' : 's'} on real requests`,
    '',
    'The new version answered real requests differently from the old one. No customer saw it: the old version served them.',
    '',
    '| Route | What differs | Pull request | Who fixes it |',
    '|---|---|---|---|',
    ...rows,
    '',
    lead,
    '',
    `<details><summary>Machine-readable verdict (${SCHEMA})</summary>`,
    '',
    '````json',
    machine,
    '````',
    '',
    '</details>',
    '',
    `${MARKER}${hex} -->`,
    '',
  ].join('\n');
}
