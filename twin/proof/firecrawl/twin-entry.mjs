import http from 'node:http';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

export async function createApp() {
  const made = [];
  const createServer = http.createServer;
  http.createServer = function kernCapture(...args) {
    const listener = args.find((arg) => typeof arg === 'function');
    if (listener) made.push(listener);
    return createServer.apply(this, args);
  };
  try {
    require('./dist/src/index.js');
  } finally {
    http.createServer = createServer;
  }
  const { initializeBlocklist } = require('./dist/src/scraper/WebScraper/utils/blocklist.js');
  const { initializeEngineForcing } = require('./dist/src/scraper/WebScraper/utils/engine-forcing.js');
  await initializeBlocklist();
  initializeEngineForcing();
  const app = made.at(-1);
  if (!app) throw new Error('firecrawl made no HTTP server while it loaded');
  return app;
}
