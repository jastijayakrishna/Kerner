'use strict';
const fs = require('fs');

const write = (line) => {
  if (!process.env.KERN_PROGRESS) return;
  try { fs.appendFileSync(process.env.KERN_PROGRESS, `${line}\n`); } catch {}
};

module.exports = class {
  onRunStart(results) { if (results && Number.isInteger(results.numTotalTestSuites)) write(`total ${results.numTotalTestSuites}`); }
  onTestResult(test) { if (test && test.path) write(`done ${Date.now()} ${test.path}`); }
  getLastError() { return undefined; }
};
