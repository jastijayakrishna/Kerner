import fs from 'node:fs';
import { blankMaps } from './drive-support.mjs';

const own = (p) => typeof p === 'string' && p.startsWith('/workspace/') && !p.includes('/node_modules/');
const readFile = fs.promises.readFile;
fs.promises.readFile = async function (file, ...rest) {
  const out = await readFile.call(this, file, ...rest);
  return own(file) && typeof out === 'string' ? blankMaps(out) : out;
};
const readFileSync = fs.readFileSync;
fs.readFileSync = function (file, ...rest) {
  const out = readFileSync.call(this, file, ...rest);
  return own(file) && typeof out === 'string' ? blankMaps(out) : out;
};
