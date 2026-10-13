// Who fixes a difference, and what they are told.

/** By default an opened door always needs a person; hosts (a payment provider, say) are named per app. */
export const DEFAULT_HUMANS = { kinds: ['door-opened'], hosts: [] };

/**
 * Whether a person, not an agent, must decide about a difference: one of its kinds is listed in humans.kinds (an opened door), or a
 * write it differs in goes to a host listed in humans.hosts.
 */
export function needsHuman(verdict, humans = {}) {
  const kinds = new Set([...DEFAULT_HUMANS.kinds, ...(humans.kinds ?? [])]);
  if (verdict.kinds.some((kind) => kinds.has(kind))) return true;
  const hosts = humans.hosts ?? [];
  return verdict.details.some((d) => d.kind === 'effects' && hosts.some((host) => String(d.old).includes(host) || String(d.new).includes(host)));
}

export const SCHEMA = 'kern-feedback/v1';
// The twin's comments carry their own marker: Kern's check-pr counts comments marked kern-feedback:v1 as rejected attempts.
const MARKER = '<!-- kern-twin:v1 head=';

/** Text from a case only ever shows inside code formatting, on one line. */
const code = (text) => `\`${String(text).replace(/`/g, "'").replace(/[\r\n]+/g, ' ').slice(0, 200)}\``;
/** In a table, a | ends a cell even inside code formatting, unless it is written \|. */
const cell = (text) => text.replace(/\|/g, '\\|');

/** The changed file a case's difference most likely comes from: the first changed line of the new code it ran. */
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

/**
 * The cases as kern-feedback/v1, the schema Kern's agent loop speaks: one violation per case. agent_fixable is true only when an agent
 * may fix every case: each one reproduces and none needs a person. Like Kern's own verdict, it holds the hints on how a person could
 * allow each difference; an agent is handed forAgent() of it.
 */
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

/**
 * The copy an agent is handed, as Kern's for_agent makes it: the instructions only, without the reason or the hints on how a person
 * could allow a difference. An agent that read them could allow its own change instead of fixing it.
 */
export function forAgent(feedback) {
  const { reason, ...rest } = feedback;
  return { ...rest, violations: feedback.violations.map(({ allow, ...violation }) => violation) };
}

/** The pull request comment: a table of the cases, what to do, the agent's copy of the verdict, and the twin's marker for `head`. */
export function commentBody(cases, head, options = {}) {
  const feedback = toFeedback(cases, options);
  const rows = cases.map((value) => `| ${cell(code(value.route))} | ${value.kinds.join(', ')} | ${value.cause ? `#${value.cause}` : value.pulls?.length ? value.pulls.map((p) => `#${p}`).join(', ') : 'unknown'} | ${!value.reproduces ? 'a person must look' : value.human ? 'a person must decide' : 'the agent can fix it'} |`);
  const lead = feedback.agent_fixable
    ? `To fix: run ${code(`npx kern-twin test --cases ${options.casesDir ?? '<cases folder>'} --candidate ${options.candidate ?? '<entry module>'}`)} and change the new code until every case answers as the old version did. If a difference is meant, say so in the pull request.`
    : 'Some of these need a person before an agent changes anything; the table says which.';
  // Backticks are escaped so nothing in the JSON can end its fence.
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
