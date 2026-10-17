import { appendFileSync } from 'node:fs';

const write = (line) => {
  if (!process.env.KERN_PROGRESS) return;
  try { appendFileSync(process.env.KERN_PROGRESS, `${line}\n`); } catch {}
};
const total = (list) => { if (Array.isArray(list)) write(`total ${list.length}`); };
const VALUES_BYTES_MAX = 1 << 20;
const VALUE_CHARS_MAX = 2000;
const text = (v) => (typeof v === 'string' ? v.slice(0, VALUE_CHARS_MAX) : null);
let valuesBytes = 0;
const recorded = new Set();
const record = (task, result) => {
  const out = process.env.KERN_VALUES;
  if (!out || recorded.has(task.id) || valuesBytes > VALUES_BYTES_MAX) return;
  const error = Array.isArray(result.errors) ? result.errors[0] : null;
  if (!error || typeof error !== 'object') return;
  recorded.add(task.id);
  const names = [];
  for (let s = task.suite; s && names.length < 64; s = s.suite) names.push(String(s.name));
  const line = JSON.stringify({ file: task.file && task.file.filepath, names: names.reverse(), name: task.name, message: text(error.message), expected: text(error.expected), actual: text(error.actual) });
  valuesBytes += line.length;
  try { appendFileSync(out, `${line}\n`); } catch {}
};

export default class {
  onInit(ctx) { this.ctx = ctx; }
  onPathsCollected(paths) { total(paths); }
  onSpecsCollected(specs) { total(specs); }
  onTestRunStart(specs) { total(specs); }
  onTestModuleEnd(module) { if (module && module.moduleId) write(`done ${Date.now()} ${module.moduleId}`); }
  onTaskUpdate(packs) {
    for (const pack of Array.isArray(packs) ? packs : []) {
      const [id, result] = pack;
      if (!result || !['pass', 'fail', 'skip'].includes(result.state)) continue;
      const task = this.ctx && this.ctx.state && this.ctx.state.idMap && this.ctx.state.idMap.get(id);
      if (task && task.type === 'test' && result.state === 'fail') { try { record(task, result); } catch {} }
      if (task && task.type === 'suite' && task.filepath && !task.suite) write(`done ${Date.now()} ${task.filepath}`);
    }
  }
}
