'use strict';
module.exports = (results) => {
  const report = {
    numTotalTests: results.numTotalTests,
    testResults: (results.testResults || []).map((file) => ({
      name: file.testFilePath,
      status: file.testExecError || file.failureMessage ? 'failed' : 'passed',
      message: (file.testExecError && file.testExecError.message) || file.failureMessage || '',
      assertionResults: (file.testResults || []).map((t) => ({ ancestorTitles: t.ancestorTitles, title: t.title, status: t.status, failureMessages: (t.failureMessages || []).slice(0, 1).map((m) => String(m).slice(0, 2000)) })),
    })),
  };
  if (process.env.KERN_REPORT) {
    try { require('fs').writeFileSync(process.env.KERN_REPORT, JSON.stringify(report)); } catch {}
  }
  process.stdout.write(`\n${JSON.stringify(report)}\n`);
  return results;
};
