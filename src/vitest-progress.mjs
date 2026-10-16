import { appendFileSync } from 'node:fs';

const write = (line) => {
  if (!process.env.KERN_PROGRESS) return;
  try { appendFileSync(process.env.KERN_PROGRESS, `${line}\n`); } catch {}
};
const total = (list) => { if (Array.isArray(list)) write(`total ${list.length}`); };

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
      if (task && task.type === 'suite' && task.filepath && !task.suite) write(`done ${Date.now()} ${task.filepath}`);
    }
  }
}
