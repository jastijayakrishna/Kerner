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

/** Production with switching: the twin's own listener serves, and sends each request to the version serving now. */
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

/** One GET, its body read to the end; answers the status. */
export const get = async (url) => {
  const answer = await fetch(url);
  await answer.arrayBuffer();
  return answer.status;
};

/** Waits until `check()` holds, looking every 50 ms, for at most `ms`. */
export async function until(check, ms = 5000) {
  const end = Date.now() + ms;
  while (!check()) {
    if (Date.now() > end) throw new Error(`still waiting after ${ms} ms`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}
