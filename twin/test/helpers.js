import http from 'node:http';
import { fileURLToPath } from 'node:url';
import { createTwin } from '../src/index.js';

/** One version of a test app, as the twin loads it: an absolute file and the function that builds the request listener. */
export const app = (name, version) => ({ module: fileURLToPath(new URL(`./apps/${name}/${version}.js`, import.meta.url)), export: 'createApp' });

/** A local HTTP server for a listener. */
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

/** Production as it would run: the old app serves customers, and the twin runs the old and the new version as ghosts. */
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
