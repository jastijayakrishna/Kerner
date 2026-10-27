'use strict';
const fs = require('fs');
const { spawnSync } = require('child_process');

const RAW_BYTES_MAX = 1024 * 1024 * 1024;
const CHUNK_BYTES = bounded(process.env.KERN_MAP_CHUNK_BYTES, 64, 64 * 1024 * 1024, 4 * 1024 * 1024);
const INDEX_METADATA = 1;
const INDEX_DOCUMENT = 2;
const METADATA_TOOL = 2;
const TOOL_NAME = 1;
const TOOL_VERSION = 2;
const DOCUMENT_PATH = 1;
const DOCUMENT_OCCURRENCE = 2;
const OCCURRENCE_RANGE = 1;
const OCCURRENCE_SYMBOL = 2;
const OCCURRENCE_ROLES = 3;
const OCCURRENCE_ENCLOSING = 7;
const DEFINITION = 1;

function bounded(text, min, max, fallback) {
  const n = Number(text);
  return Number.isInteger(n) ? Math.min(max, Math.max(min, n)) : fallback;
}

class Incomplete extends Error {}

function varint(b, i, end) {
  let r = 0, s = 1;
  for (;;) {
    if (i >= end) throw new Incomplete();
    const c = b[i++];
    r += (c & 0x7f) * s;
    if (c < 0x80) return [r, i];
    s *= 128;
  }
}

function field(b, i, end) {
  const [tag, j] = varint(b, i, end);
  const number = Math.floor(tag / 8), wire = tag % 8;
  if (wire === 0) { const [value, k] = varint(b, j, end); return { number, value, start: j, end: k, next: k }; }
  if (wire === 1) { if (j + 8 > end) throw new Incomplete(); return { number, value: 0, start: j, end: j + 8, next: j + 8 }; }
  if (wire === 5) { if (j + 4 > end) throw new Incomplete(); return { number, value: 0, start: j, end: j + 4, next: j + 4 }; }
  if (wire === 2) { const [len, k] = varint(b, j, end); if (k + len > end) throw new Incomplete(); return { number, value: 0, start: k, end: k + len, next: k + len }; }
  throw new Error(`unreadable index: wire type ${wire} at byte ${i}`);
}

function fields(b, start, end, each) {
  let i = start;
  while (i < end) { const f = field(b, i, end); each(f.number, f.start, f.end, f.value); i = f.next; }
}

function packedInts(b, start, end) {
  const out = [];
  let i = start;
  while (i < end) { const [v, j] = varint(b, i, end); out.push(v); i = j; }
  return out;
}

function occurrence(b, start, end, symbols) {
  let symbol = '', roles = 0, range = [], enclosing = [];
  fields(b, start, end, (number, s, e, value) => {
    if (number === OCCURRENCE_SYMBOL) symbol = b.toString('utf8', s, e);
    else if (number === OCCURRENCE_ROLES) roles = value;
    else if (number === OCCURRENCE_RANGE) range = packedInts(b, s, e);
    else if (number === OCCURRENCE_ENCLOSING) enclosing = packedInts(b, s, e);
  });
  if (symbol === '' || symbol.startsWith('local ') || range.length === 0) return null;
  let id = symbols.get(symbol);
  if (id === undefined) { id = symbols.size; symbols.set(symbol, id); }
  if ((roles & DEFINITION) === 0) return [id, roles, range[0], range[0]];
  const r = enclosing.length ? enclosing : range;
  return [id, roles, r[0], r.length === 4 ? Math.max(r[2], r[0]) : r[0]];
}

function document(b, start, end, symbols) {
  let path = '';
  const occ = [];
  fields(b, start, end, (number, s, e) => {
    if (number === DOCUMENT_PATH) path = b.toString('utf8', s, e);
    else if (number === DOCUMENT_OCCURRENCE) { const o = occurrence(b, s, e, symbols); if (o) occ.push(o); }
  });
  return { path, occ };
}

function tool(b, start, end) {
  let name = '', version = '';
  fields(b, start, end, (number, s, e) => {
    if (number !== METADATA_TOOL) return;
    fields(b, s, e, (n, s2, e2) => {
      if (n === TOOL_NAME) name = b.toString('utf8', s2, e2);
      else if (n === TOOL_VERSION) version = b.toString('utf8', s2, e2);
    });
  });
  return name ? `${name} ${version}`.trim() : 'unknown indexer';
}

function* topLevelFields(fd) {
  let buf = Buffer.alloc(0), pos = 0, eof = false;
  for (;;) {
    let f = null;
    try { if (pos < buf.length) f = field(buf, pos, buf.length); } catch (e) { if (!(e instanceof Incomplete)) throw e; }
    if (f) { yield [f.number, buf, f.start, f.end]; pos = f.next; continue; }
    if (eof) { if (pos < buf.length) throw new Error('unreadable index: it ends inside a field'); return; }
    const chunk = Buffer.alloc(CHUNK_BYTES);
    const got = fs.readSync(fd, chunk, 0, CHUNK_BYTES, null);
    if (got === 0) { eof = true; continue; }
    buf = pos === 0 && buf.length === 0 ? chunk.subarray(0, got) : Buffer.concat([buf.subarray(pos), chunk.subarray(0, got)]);
    pos = 0;
  }
}

function compact(raw, output) {
  const symbols = new Map();
  const out = fs.openSync(output, 'w');
  const fd = fs.openSync(raw, 'r');
  let provenance = 'unknown indexer', documents = 0, bytes = 0;
  try {
    for (const [number, b, s, e] of topLevelFields(fd)) {
      if (number === INDEX_METADATA) provenance = tool(b, s, e);
      else if (number === INDEX_DOCUMENT) { documents++; bytes += fs.writeSync(out, JSON.stringify(document(b, s, e, symbols)) + '\n'); }
    }
    bytes += fs.writeSync(out, JSON.stringify({ version: 1, tool: provenance, symbols: [...symbols.keys()] }) + '\n');
  } finally {
    fs.closeSync(fd);
    fs.closeSync(out);
  }
  return [documents, bytes];
}

function write(raw, output) {
  const size = fs.statSync(raw).size;
  if (size > RAW_BYTES_MAX) { process.stderr.write(`map.cjs: the index is ${size} bytes, more than Kern reads (${RAW_BYTES_MAX})\n`); return 1; }
  const [documents, bytes] = compact(raw, output);
  process.stderr.write(`map.cjs: ${documents} documents; ${size} bytes indexed, ${bytes} kept as symbols, definitions and references\n`);
  return 0;
}

function main() {
  const args = process.argv.slice(2);
  if (args[0] === '--compact' && args.length === 3) return write(args[1], args[2]);
  const [indexer, ...flags] = args;
  const at = flags.indexOf('--output');
  if (!indexer || at < 0 || at + 1 >= flags.length) { process.stderr.write('map.cjs: usage: map.cjs <indexer main.js> index ... --output <file>  |  map.cjs --compact <index.scip> <facts.jsonl>\n'); return 2; }
  const output = flags[at + 1];
  const raw = output + '.raw';
  flags[at + 1] = raw;
  const run = spawnSync(process.execPath, [...process.execArgv, indexer, ...flags], { stdio: 'inherit' });
  if (run.status !== 0) { process.stderr.write(`map.cjs: the indexer stopped (${run.status === null ? run.signal : 'exit ' + run.status})\n`); return run.status === null ? 1 : run.status; }
  try { return write(raw, output); } finally { fs.unlinkSync(raw); }
}

process.exitCode = main();
