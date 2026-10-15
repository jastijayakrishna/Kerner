import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { answer, ask, ghostEnv, stopped } from './ghost-common.js';

const MAIN = fileURLToPath(new URL('./process-ghost-main.js', import.meta.url));
const PERMISSION = process.allowedNodeEnvironmentFlags.has('--permission') ? '--permission' : '--experimental-permission';
export const BWRAP = ['--unshare-user', '--unshare-net', '--unshare-ipc', '--unshare-uts', '--unshare-pid', '--die-with-parent', '--new-session',
  '--ro-bind', '/', '/', '--dev', '/dev', '--proc', '/proc', '--tmpfs', '/tmp'];

export function ghostCommand(settings) {
  const node = [`--max-old-space-size=${settings.memoryMb}`, ...(settings.ghostExecArgv ?? [])];
  if (settings.sandbox !== 'bwrap') {
    node.push(PERMISSION, '--allow-fs-read=*');
    if (settings.addons?.length) node.push('--allow-addons');
  }
  node.push(MAIN);
  return settings.sandbox === 'bwrap' ? ['bwrap', [...BWRAP, process.execPath, ...node]] : [process.execPath, node];
}

function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((resolve) => {
    child.once('exit', () => resolve());
    child.kill('SIGKILL');
  });
}

export function startProcessGhost(side, settings) {
  return new Promise((resolve, reject) => {
    const [command, args] = ghostCommand(settings);
    let child;
    try {
      child = spawn(command, args, { stdio: ['ignore', 'ignore', 'ignore', 'ipc'], serialization: 'advanced', env: ghostEnv(), windowsHide: true });
    } catch (error) {
      reject(error);
      return;
    }
    let timer = null;
    let loadLines = [];
    const cleanup = () => {
      clearTimeout(timer);
      child.off('message', onMessage);
      child.off('error', onError);
      child.off('exit', onExit);
    };
    const fail = (error) => {
      cleanup();
      child.on('error', () => {});
      stop(child);
      reject(error);
    };
    const onMessage = (message) => {
      if (message?.type === 'load-lines') loadLines = message.lines;
      if (message?.type === 'failed') fail(new Error(message.reason));
      if (message?.type !== 'ready') return;
      cleanup();
      const dirty = [];
      child.on('error', (error) => dirty.forEach((listener) => listener({ type: 'dirty', error: `the ghost failed: ${error?.message ?? error}` })));
      child.on('exit', (code, signal) => dirty.forEach((listener) => listener({ type: 'dirty', error: `the ghost stopped (exit ${code ?? signal})` })));
      child.on('message', (m) => {
        if (m?.type === 'dirty') dirty.forEach((listener) => listener(m));
        else if (m?.type === 'real-read') answer((reply) => child.connected && child.send(reply), m, settings);
      });
      resolve({
        kind: 'process',
        loadLines,
        ask: (id, job, timeoutMs) => ask(child, (m) => child.connected && child.send(m), id, job, timeoutMs),
        terminate: () => stop(child),
        onDirty: (listener) => dirty.push(listener),
        stopped: () => stopped(child),
      });
    };
    const onError = (error) => fail(new Error(`could not start ${command} (${error.code ?? error.message})`));
    const onExit = (code, signal) => fail(new Error(`stopped while loading (exit ${code ?? signal})`));
    timer = setTimeout(() => fail(new Error(`did not load within ${settings.startMs} ms`)), settings.startMs);
    child.on('message', onMessage);
    child.on('error', onError);
    child.on('exit', onExit);
    child.send({
      type: 'start',
      side: {
        module: side.module, export: side.export, addons: settings.addons ?? [], coverage: settings.coverage ?? null,
        adapters: settings.adapters ?? [], overlay: Boolean(settings.overlay), realReads: settings.realReads ?? [],
        load: settings.load ?? null, pgReads: Boolean(settings.pgReads), maxBodyBytes: settings.maxBodyBytes ?? null,
      },
    });
  });
}
