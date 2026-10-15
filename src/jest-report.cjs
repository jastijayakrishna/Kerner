'use strict';
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
