const files = new Map();
const errs = new Map();
const slash = (p) => String(p).replace(/\\/g, '/').replace(/^\.\//, '');
const of = (file) => {
  if (!files.has(file)) files.set(file, { stack: [], parents: [], tests: [], failure: '' });
  return files.get(file);
};
const printed = (file) => {
  const full = slash(file);
  for (const [k, v] of errs) if (k === full || full.endsWith('/' + k)) return v;
  return '';
};
const message = (error) => {
  if (!error) return '';
  const cause = error.cause && typeof error.cause === 'object' ? error.cause : error;
  return String((cause && (cause.stack || cause.message)) || error.message || error).slice(0, 2000);
};

export default async function* report(source) {
  for await (const { type, data } of source) {
    if (type === 'test:stderr' && data && data.file && typeof data.message === 'string') {
      const k = slash(data.file);
      const was = errs.get(k) || '';
      if (was.length < 4000) errs.set(k, was + data.message);
      continue;
    }
    if (!data || typeof data.nesting !== 'number') continue;
    const file = data.file || '';
    const f = of(file);
    if (type === 'test:start') {
      f.stack.length = data.nesting;
      f.parents.length = data.nesting;
      f.stack[data.nesting] = data.name;
      if (data.nesting > 0) f.parents[data.nesting - 1] = true;
      f.parents[data.nesting] = false;
      continue;
    }
    if (type !== 'test:pass' && type !== 'test:fail') continue;
    const suite = data.details && data.details.type === 'suite';
    const hasChildren = f.parents[data.nesting] === true;
    const failed = type === 'test:fail';
    const loading = failed && data.nesting === 0 && file && !hasChildren && (slash(file) === slash(data.name) || slash(file).endsWith('/' + slash(data.name)));
    if (loading) { f.failure = (printed(file).trim() || message(data.details && data.details.error) || 'the test file failed').slice(0, 2000); continue; }
    if (suite || (hasChildren && !failed)) continue;
    const ancestors = f.stack.slice(0, data.nesting).filter((n) => typeof n === 'string');
    const status = data.skip !== undefined && data.skip !== false ? 'skipped' : data.todo !== undefined && data.todo !== false ? 'todo' : failed ? 'failed' : 'passed';
    f.tests.push({ ancestorTitles: ancestors, title: data.name, status, failureMessages: failed ? [message(data.details && data.details.error)] : [] });
  }
  const testResults = [...files.entries()].filter(([, f]) => f.tests.length > 0 || f.failure).map(([name, f]) => ({
    name,
    status: f.failure || f.tests.some((t) => t.status === 'failed') ? 'failed' : 'passed',
    message: f.failure,
    assertionResults: f.tests,
  }));
  yield JSON.stringify({ numTotalTests: testResults.reduce((n, r) => n + r.assertionResults.length, 0), testResults });
}
