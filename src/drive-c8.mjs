// Kern's preload for its coverage report (c8). For a script Node made no source map for, c8 reads the script's own file and takes that
// file's sourceMappingURL comment as its map, so a project file could still place code that never ran on any line it likes: c8's reads
// of project source files see those comments blanked, as the drive loader blanks them (drive-support.mjs).
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
