import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HELPERS = new Set(['__name', '__export', '__copyProps', '__toCommonJS', '__toESM', '__reExport', '__commonJS', '__esm', '__require', '__defProp',
  '__defNormalProp', '__publicField', '__decorateClass', '__decorateParam', '__async', '__await', '__spreadValues', '__spreadProps', '__objRest',
  '__template', '__using', 'get']);
const QUIET = new Set([...'{}()[];,']);
export const isCode = (c) => c.trim() !== '' && !QUIET.has(c);
const normalize = (path) => (process.platform === 'win32' ? resolve(path).toLowerCase() : resolve(path));

export function plausible(text, column, isFunction) {
  const before = text.slice(0, column).trimEnd();
  const at = text.slice(column).trimStart();
  if (at.startsWith('export') || at.startsWith('import')) return false;
  if (isFunction) {
    const name = /^[\p{L}\p{N}_$]*/u.exec(at)[0];
    const rest = at.slice(name.length).trimStart();
    return ['function', 'async', '(', 'get ', 'set ', 'static ', '*', '#'].some((s) => at.startsWith(s))
      || (name !== '' && !['const', 'let', 'var', 'return', 'if', 'for', 'while', 'switch', 'new', 'await', 'typeof'].includes(name) && (rest.startsWith('=>') || rest.startsWith('(')));
  }
  return before === '' || at.startsWith('{') || ['?', ':', '&&', '||', '??', ')', 'else', '=>', '{', ';'].some((t) => before.endsWith(t))
    || [':', '&&', '||', '??'].some((t) => at.startsWith(t)) || (at.startsWith('?') && !at.startsWith('?.'));
}

function lineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) starts.push(i + 1);
  return starts;
}

function lineEnd(text, starts, line) {
  let end = line + 1 < starts.length ? starts[line + 1] - 1 : text.length;
  if (end > starts[line] && text[end - 1] === '\r') end--;
  return end;
}

function lineAt(starts, offset) {
  let low = 0;
  let high = starts.length - 1;
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if (starts[mid] <= offset) low = mid;
    else high = mid - 1;
  }
  return low;
}

function spansOf(text, starts, functions) {
  const spans = [];
  for (const f of functions) {
    if (HELPERS.has(f.functionName)) continue;
    f.ranges.forEach((range, i) => {
      if (range.count === 0) {
        const line = lineAt(starts, range.startOffset);
        if (!plausible(text.slice(starts[line], lineEnd(text, starts, line)), range.startOffset - starts[line], i === 0)) return;
      }
      spans.push({ start: range.startOffset, end: range.endOffset, count: range.count });
    });
  }
  return spans;
}

function codeOffsets(text, from, to) {
  const out = [];
  for (let i = from; i < to; i++) if (isCode(text[i])) out.push(i);
  return out;
}

function maskOf(spans, offsets) {
  let mask = '';
  for (const at of offsets) {
    let inner = null;
    for (const s of spans) if (s.start <= at && at < s.end && (!inner || s.end - s.start <= inner.end - inner.start)) inner = s;
    mask += inner && inner.count > 0 ? '1' : '0';
  }
  return mask;
}

export function lineMasks(text, functions, lines) {
  const starts = lineStarts(text);
  const spans = spansOf(text, starts, functions);
  const out = new Map();
  for (const line of lines) {
    if (line < 1 || line > starts.length) continue;
    const from = starts[line - 1];
    const to = lineEnd(text, starts, line - 1);
    out.set(line, maskOf(spans.filter((s) => s.start < to && s.end > from), codeOffsets(text, from, to)));
  }
  return out;
}

const DIGITS = new Map([...'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'].map((c, i) => [c, i]));

export function decodeMappings(mappings) {
  const lines = [];
  let source = 0;
  let line = 0;
  let column = 0;
  for (const text of String(mappings).split(';')) {
    const segments = [];
    let generated = 0;
    for (const piece of text.split(',')) {
      if (!piece) continue;
      const values = [];
      let value = 0;
      let shift = 0;
      for (const ch of piece) {
        const digit = DIGITS.get(ch);
        if (digit === undefined) throw new Error('not a source map');
        value += (digit & 31) * 2 ** shift;
        if (digit & 32) shift += 5;
        else {
          values.push(value % 2 ? -Math.floor(value / 2) : value / 2);
          value = 0;
          shift = 0;
        }
      }
      generated += values[0];
      if (values.length >= 4) {
        source += values[1];
        line += values[2];
        column += values[3];
        segments.push([generated, source, line, column]);
      } else segments.push([generated, -1, -1, -1]);
    }
    lines.push(segments);
  }
  return lines;
}

export function sourceMapOf(text, file) {
  const at = text.lastIndexOf('sourceMappingURL=');
  if (at < 0) return null;
  const url = text.slice(at + 'sourceMappingURL='.length).split(/\s/, 1)[0];
  try {
    let found;
    if (url.startsWith('data:')) {
      const comma = url.indexOf(',');
      const json = url.slice(0, comma).endsWith(';base64') ? Buffer.from(url.slice(comma + 1), 'base64').toString('utf8') : decodeURIComponent(url.slice(comma + 1));
      found = { map: JSON.parse(json), dir: dirname(file) };
    } else {
      const mapFile = resolve(dirname(file), decodeURIComponent(url));
      found = { map: JSON.parse(readFileSync(mapFile, 'utf8')), dir: dirname(mapFile) };
    }
    return typeof found.map?.mappings === 'string' && Array.isArray(found.map.sources) ? found : null;
  } catch {
    return null;
  }
}

function sourcesOf(map, dir) {
  const root = typeof map.sourceRoot === 'string' ? map.sourceRoot : '';
  return map.sources.map((source) => {
    if (typeof source !== 'string') return '';
    if (source.startsWith('file:')) {
      try {
        return normalize(fileURLToPath(source));
      } catch {
        return '';
      }
    }
    return normalize(resolve(dir, root, source));
  });
}

export function mappedMasks(text, functions, map, mapDir, targets) {
  const starts = lineStarts(text);
  const spans = spansOf(text, starts, functions);
  const sources = sourcesOf(map, mapDir);
  const wanted = new Map();
  sources.forEach((source, index) => {
    const target = targets.get(source);
    if (target) wanted.set(index, new Set(target.lines));
  });
  const pieces = new Map();
  decodeMappings(map.mappings).forEach((segments, generated) => {
    if (generated >= starts.length) return;
    const lineStart = starts[generated];
    const end = lineEnd(text, starts, generated);
    segments.forEach(([column, source, original], i) => {
      if (source < 0 || !wanted.get(source)?.has(original + 1)) return;
      const from = lineStart + column;
      const to = Math.min(end, i + 1 < segments.length ? lineStart + segments[i + 1][0] : end);
      if (from >= to) return;
      const key = `${source}:${original + 1}`;
      if (!pieces.has(key)) pieces.set(key, []);
      pieces.get(key).push([from, to]);
    });
  });
  const out = new Map();
  for (const [index, lines] of wanted) {
    const masks = new Map();
    for (const line of lines) {
      const list = pieces.get(`${index}:${line}`);
      if (!list) continue;
      const near = spans.filter((s) => list.some(([from, to]) => s.start < to && s.end > from));
      masks.set(line, maskOf(near, list.flatMap(([from, to]) => codeOffsets(text, from, to))));
    }
    out.set(sources[index], masks);
  }
  return out;
}

export function targetsOf(plan) {
  return new Map(plan.files.map((file) => [normalize(resolve(plan.root, file.path)), { path: file.path, lines: file.lines }]));
}

export async function startCoverage(session) {
  await session.post('Profiler.enable');
  await session.post('Profiler.startPreciseCoverage', { callCount: true, detailed: true });
  await session.post('Debugger.enable');
}

function sameLines(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if ((a.charCodeAt(i) === 10) !== (b.charCodeAt(i) === 10)) return false;
  return true;
}

async function howToRead(session, script, targets) {
  let file;
  try {
    file = normalize(fileURLToPath(script.url));
  } catch {
    return null;
  }
  let text;
  try {
    ({ scriptSource: text } = await session.post('Debugger.getScriptSource', { scriptId: script.scriptId }));
  } catch {
    return null;
  }
  const target = targets.get(file);
  if (target) {
    let disk = null;
    try {
      disk = readFileSync(file, 'utf8');
    } catch {
    }
    if (disk !== null && sameLines(disk, text)) return { file, text, lines: target.lines };
  }
  const found = sourceMapOf(text, file);
  if (!found || !sourcesOf(found.map, found.dir).some((source) => targets.has(source))) return null;
  return { text, map: found.map, mapDir: found.dir };
}

export async function takeLines(coverage) {
  const { session, side, targets, cache } = coverage;
  const { result } = await session.post('Profiler.takePreciseCoverage');
  const out = {};
  for (const script of result) {
    if (!script.url.startsWith('file:') || script.url.includes('/node_modules/')) continue;
    if (!cache.has(script.scriptId)) cache.set(script.scriptId, await howToRead(session, script, targets));
    const how = cache.get(script.scriptId);
    if (!how) continue;
    const masks = how.map ? mappedMasks(how.text, script.functions, how.map, how.mapDir, targets) : new Map([[how.file, lineMasks(how.text, script.functions, how.lines)]]);
    for (const [file, lines] of masks) {
      const path = targets.get(file).path;
      for (const [line, mask] of lines) if (mask.includes('1')) out[`${side}:${path}:${line}`] = mask;
    }
  }
  return out;
}
