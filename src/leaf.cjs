'use strict';
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const HOOK = `--require=${__filename}`;
const RUNNERS = { jest: 'jest', vitest: 'vitest', mocha: 'mocha', _mocha: 'mocha' };
const PACKAGES = ['jest', 'jest-cli', 'vitest', 'mocha'];
const PACKAGE_DEPTH_MAX = 4;
const PACKAGE_BYTES_MAX = 1 << 20;
const ARGS_COUNT_MAX = 512;
const ARG_CHARS_MAX = 8192;
const ENV_COUNT_MAX = 128;
const VALUE_CHARS_MAX = 4096;
const EVENT_CHARS_MAX = 256;
const RECORD_CHARS_MAX = 1 << 18;
const NAME = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;
const SKIPPED = /^(npm_|kern_|path$)/i;

const declared = (manifest, dir, file) => {
  try {
    if (fs.statSync(manifest).size > PACKAGE_BYTES_MAX) return null;
    const pkg = JSON.parse(fs.readFileSync(manifest, 'utf8'));
    if (!PACKAGES.includes(pkg.name)) return null;
    const bins = typeof pkg.bin === 'string' ? { [pkg.name]: pkg.bin } : pkg.bin || {};
    for (const [name, bin] of Object.entries(bins)) {
      if (RUNNERS[name] && typeof bin === 'string' && path.resolve(dir, bin) === file) return RUNNERS[name];
    }
  } catch {}
  return null;
};

const runner = () => {
  if (!process.argv[1]) return null;
  let file;
  try { file = fs.realpathSync(process.argv[1]); } catch { return null; }
  let dir = path.dirname(file);
  for (let depth = 0; depth < PACKAGE_DEPTH_MAX; depth += 1) {
    const manifest = path.join(dir, 'package.json');
    if (fs.existsSync(manifest)) return declared(manifest, dir, file);
    const up = path.dirname(dir);
    if (up === dir) return null;
    dir = up;
  }
  return null;
};

const ours = (option) => {
  const at = option.indexOf('=');
  return at > 0 && ['--require', '-r'].includes(option.slice(0, at)) && path.resolve(option.slice(at + 1)) === __filename;
};
const own = (options) => options.split(/\s+/).filter((o) => o && !ours(o)).join(' ');

const environment = () => {
  const names = Object.keys(process.env).filter((k) => NAME.test(k) && !SKIPPED.test(k)).sort();
  if (names.length > ENV_COUNT_MAX) return null;
  const out = {};
  for (const k of names) {
    const v = k === 'NODE_OPTIONS' ? own(process.env[k]) : process.env[k];
    if (v.length > VALUE_CHARS_MAX) return null;
    if (k !== 'NODE_OPTIONS' || v) out[k] = v;
  }
  return out;
};

const fits = (list) => list.length <= ARGS_COUNT_MAX && list.every((a) => a.length <= ARG_CHARS_MAX);

const record = (file, name) => {
  const env = environment();
  const argv = process.argv.slice(2);
  const node = process.execArgv.filter((a) => !ours(a));
  const event = String(process.env.npm_lifecycle_event || '').slice(0, EVENT_CHARS_MAX);
  const whole = env !== null && fits(argv) && fits(node);
  let line = JSON.stringify(whole ? { runner: name, cwd: process.cwd(), argv, node, env, event } : { runner: name, cwd: process.cwd(), cut: true });
  if (line.length > RECORD_CHARS_MAX) line = JSON.stringify({ runner: name, cut: true });
  fs.appendFileSync(file, `${line}\n`);
};

const hook = () => {
  const file = process.env.KERN_LEAVES;
  if (!file) return;
  const name = runner();
  if (!name) return;
  try { record(file, name); } catch (e) {
    try { process.stderr.write(`kern: could not record a ${name} run: ${e.message}\n`); } catch {}
    process.exit(70);
  }
  process.exit(0);
};

const drive = ([command, ...args]) => {
  const out = process.env.KERN_REPORT;
  const env = { ...process.env, KERN_LEAVES: out, NODE_OPTIONS: HOOK, PATH: `${path.join(__dirname, 'bin')}${path.delimiter}${process.env.PATH || ''}` };
  let done = false;
  const ended = (exit, signal, error) => {
    if (done) return;
    done = true;
    fs.appendFileSync(out, `${JSON.stringify({ end: true, exit, signal, error })}\n`);
    process.exit(0);
  };
  const child = spawn(command, args, { stdio: 'inherit', env });
  child.on('error', (e) => ended(null, null, String(e.message).slice(0, VALUE_CHARS_MAX)));
  child.on('exit', (code, signal) => ended(code, signal, null));
};

if (require.main === module) drive(process.argv.slice(2)); else hook();
