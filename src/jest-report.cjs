'use strict';
// Kern reads jest's results through this processor instead of --json. --json also sends the tests' console to stderr, which changes what
// tests that capture output see (three of winston's tests fail under --json and pass without it). The report carries the fields of --json.
module.exports = (results) => {
  const report = {
    numTotalTests: results.numTotalTests,
    testResults: (results.testResults || []).map((file) => ({
      name: file.testFilePath,
      status: file.testExecError || file.failureMessage ? 'failed' : 'passed',
      assertionResults: (file.testResults || []).map((t) => ({ ancestorTitles: t.ancestorTitles, title: t.title, status: t.status })),
    })),
  };
  process.stdout.write(`\n${JSON.stringify(report)}\n`);
  return results;
};
