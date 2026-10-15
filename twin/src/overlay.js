import fs from 'node:fs';
import { Writable } from 'node:stream';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

let entries = new Map();

export function resetOverlay() {
  entries = new Map();
}

const pathOf = (target) => resolve(target instanceof URL ? fileURLToPath(target) : String(target));
const missing = (syscall, path) => Object.assign(new Error(`ENOENT: no such file or directory, ${syscall} '${path}'`), { code: 'ENOENT', errno: -2, syscall, path });

function statsOf(entry) {
  const now = new Date();
  const time = now.getTime();
  const size = entry.kind === 'file' ? entry.data.length : 0;
  return {
    isFile: () => entry.kind === 'file', isDirectory: () => entry.kind === 'dir', isSymbolicLink: () => false, isFIFO: () => false,
    isSocket: () => false, isBlockDevice: () => false, isCharacterDevice: () => false,
    size, mode: entry.kind === 'dir' ? 0o40755 : 0o100644, uid: 0, gid: 0, nlink: 1, dev: 0, ino: 0, rdev: 0, blksize: 4096, blocks: Math.ceil(size / 512),
    atime: now, mtime: now, ctime: now, birthtime: now, atimeMs: time, mtimeMs: time, ctimeMs: time, birthtimeMs: time,
  };
}

function bytesOf(data, options) {
  if (Buffer.isBuffer(data)) return Buffer.from(data);
  if (data instanceof Uint8Array) return Buffer.from(data.buffer, data.byteOffset, data.byteLength);
  const encoding = typeof options === 'string' ? options : options?.encoding ?? 'utf8';
  return Buffer.from(String(data), encoding);
}

function decoded(data, options) {
  const encoding = typeof options === 'string' ? options : options?.encoding;
  return encoding ? data.toString(encoding) : Buffer.from(data);
}

export function installOverlay({ wrote }) {
  const real = { readFileSync: fs.readFileSync, existsSync: fs.existsSync, statSync: fs.statSync, lstatSync: fs.lstatSync, accessSync: fs.accessSync, readdirSync: fs.readdirSync };
  const entry = (path) => entries.get(path);
  const exists = (path) => {
    const found = entry(path);
    return found ? found.kind !== 'gone' : real.existsSync(path);
  };
  const contents = (path, syscall) => {
    const found = entry(path);
    if (found?.kind === 'file') return found.data;
    if (found?.kind === 'gone') throw missing(syscall, path);
    return real.readFileSync(path);
  };

  const writes = {
    writeFile(target, data, options) {
      const path = pathOf(target);
      const append = typeof options === 'object' && String(options?.flag ?? '').startsWith('a');
      const before = append && exists(path) ? contents(path, 'open') : Buffer.alloc(0);
      entries.set(path, { kind: 'file', data: Buffer.concat([before, bytesOf(data, options)]) });
    },
    appendFile(target, data, options) {
      const path = pathOf(target);
      const before = exists(path) ? contents(path, 'open') : Buffer.alloc(0);
      entries.set(path, { kind: 'file', data: Buffer.concat([before, bytesOf(data, options)]) });
    },
    mkdir(target, options) {
      let path = pathOf(target);
      const made = [];
      while (!exists(path)) {
        made.push(path);
        if (!options?.recursive) break;
        const up = dirname(path);
        if (up === path) break;
        path = up;
      }
      for (const dir of made) entries.set(dir, { kind: 'dir' });
      return options?.recursive ? made.at(-1) : undefined;
    },
    rm(target, options) {
      const path = pathOf(target);
      if (!exists(path)) {
        if (options?.force) return;
        throw missing('rm', path);
      }
      for (const key of [...entries.keys()]) if (key.startsWith(`${path}/`) || key.startsWith(`${path}\\`)) entries.set(key, { kind: 'gone' });
      entries.set(path, { kind: 'gone' });
    },
    rmdir(target) {
      const path = pathOf(target);
      if (!exists(path)) throw missing('rmdir', path);
      entries.set(path, { kind: 'gone' });
    },
    unlink(target) {
      const path = pathOf(target);
      if (!exists(path)) throw missing('unlink', path);
      entries.set(path, { kind: 'gone' });
    },
    rename(from, to) {
      const source = pathOf(from);
      if (!exists(source)) throw missing('rename', source);
      const found = entry(source);
      const isDir = found ? found.kind === 'dir' : real.statSync(source).isDirectory();
      entries.set(pathOf(to), isDir ? { kind: 'dir' } : { kind: 'file', data: contents(source, 'rename') });
      entries.set(source, { kind: 'gone' });
    },
    copyFile(from, to) {
      entries.set(pathOf(to), { kind: 'file', data: Buffer.from(contents(pathOf(from), 'copyfile')) });
    },
    truncate(target, length = 0) {
      const path = pathOf(target);
      const data = contents(path, 'open');
      entries.set(path, { kind: 'file', data: Buffer.concat([data, Buffer.alloc(Math.max(0, length - data.length))]).subarray(0, length) });
    },
  };
  const reads = {
    readFile(target, options) {
      if (typeof target === 'number') return real.readFileSync(target, options);
      return decoded(contents(pathOf(target), 'open'), options);
    },
    stat(target, options) {
      const path = pathOf(target);
      const found = entry(path);
      if (found?.kind === 'gone') throw missing('stat', path);
      return found ? statsOf(found) : real.statSync(path, options);
    },
    lstat(target, options) {
      const path = pathOf(target);
      const found = entry(path);
      if (found?.kind === 'gone') throw missing('lstat', path);
      return found ? statsOf(found) : real.lstatSync(path, options);
    },
    access(target, mode) {
      const path = pathOf(target);
      const found = entry(path);
      if (found?.kind === 'gone') throw missing('access', path);
      if (!found) real.accessSync(path, mode);
    },
    readdir(target, options) {
      const dir = pathOf(target);
      const own = entry(dir);
      if (own?.kind === 'gone') throw missing('scandir', dir);
      let names = [];
      try {
        names = real.readdirSync(dir);
      } catch (error) {
        if (own?.kind !== 'dir') throw error;
      }
      const gone = new Set();
      const added = new Set();
      for (const [path, found] of entries) if (path !== dir && dirname(path) === dir) (found.kind === 'gone' ? gone : added).add(basename(path));
      const list = [...new Set([...names.filter((name) => !gone.has(name)), ...added])].sort();
      if (!options?.withFileTypes) return list;
      return list.map((name) => {
        const path = join(dir, name);
        const found = entry(path);
        const stats = found ? statsOf(found) : real.statSync(path);
        return { name, parentPath: dir, path: dir, isFile: () => stats.isFile(), isDirectory: () => stats.isDirectory(), isSymbolicLink: () => false };
      });
    },
  };

  const callbackOf = (args) => (typeof args.at(-1) === 'function' ? args.at(-1) : null);
  const argsOf = (args) => (typeof args.at(-1) === 'function' ? args.slice(0, -1) : args);
  const effect = (name, target) => wrote(`FS ${name} ${pathOf(target)} -`);
  const forms = (name, op, write) => {
    fs[`${name}Sync`] = (...args) => {
      if (write) effect(name, args[0]);
      return op(...args);
    };
    fs[name] = (...args) => {
      const done = callbackOf(args);
      if (write) effect(name, args[0]);
      let out;
      try {
        out = op(...argsOf(args));
      } catch (error) {
        if (done) process.nextTick(done, error);
        return;
      }
      if (done) process.nextTick(done, null, out);
    };
    fs.promises[name] = async (...args) => {
      if (write) effect(name, args[0]);
      return op(...args);
    };
  };
  for (const [name, op] of Object.entries(writes)) forms(name, op, true);
  for (const [name, op] of Object.entries(reads)) forms(name, op, false);
  fs.existsSync = (target) => {
    try {
      return exists(pathOf(target));
    } catch {
      return false;
    }
  };
  fs.createWriteStream = (target) => {
    const path = pathOf(target);
    effect('createWriteStream', target);
    entries.set(path, { kind: 'file', data: Buffer.alloc(0) });
    return new Writable({
      write(chunk, encoding, callback) {
        const found = entries.get(path);
        const before = found?.kind === 'file' ? found.data : Buffer.alloc(0);
        entries.set(path, { kind: 'file', data: Buffer.concat([before, Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk, encoding)]) });
        callback();
      },
    });
  };
}
