import http from 'node:http';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../src/index.js';
import { join } from 'node:path';

export const app = (name, version) => ({ module: fileURLToPath(new URL(`./apps/${name}/${version}.js`, import.meta.url)), export: 'createApp' });

export async function listen(listener) {
  const server = http.createServer(listener);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    close: () => new Promise((resolve) => {
      server.closeAllConnections();
      server.close(resolve);
    }),
  };
}

export async function production(name, options = {}) {
  const twin = await createTwin({ old: app(name, 'old'), candidate: app(name, 'new'), timeoutMs: 3000, ...options });
  const { createApp } = await import(new URL(`./apps/${name}/old.js`, import.meta.url).href);
  const server = await listen(twin.wrap(await createApp()));
  return {
    twin,
    url: server.url,
    close: async () => {
      await server.close();
      await twin.close();
    },
  };
}

export const post = (url, value) => fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(value) });

export async function routed(name, options = {}) {
  const twin = await createTwin({ old: app(name, 'old'), candidate: app(name, 'new'), timeoutMs: 3000, ...options });
  const server = await listen(await twin.listener());
  return {
    twin,
    url: server.url,
    close: async () => {
      await server.close();
      await twin.close();
    },
  };
}

export const get = async (url) => {
  const answer = await fetch(url);
  await answer.arrayBuffer();
  return answer.status;
};

export async function until(check, ms = 5000) {
  const end = Date.now() + ms;
  while (!check()) {
    if (Date.now() > end) throw new Error(`still waiting after ${ms} ms`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

export const shopVersion = (version) => {
  const root = fileURLToPath(new URL(`./apps/three-prs/${version}/`, import.meta.url));
  return { module: join(root, 'src', 'app.js'), export: 'createApp', root };
};

export const shopManifest = fileURLToPath(new URL('./apps/three-prs/manifest.json', import.meta.url));

export async function threePrs(options = {}) {
  const twin = await createTwin({ old: shopVersion('base'), candidate: shopVersion('head'), manifest: shopManifest, timeoutMs: 3000, ...options });
  const server = await listen(await twin.listener());
  return {
    twin,
    url: server.url,
    close: async () => {
      await server.close();
      await twin.close();
    },
  };
}
