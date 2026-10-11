// firecrawl's API as a request listener, for the Kern twin. Its src/index.ts builds the Express app when it is loaded and listens only
// when it is run as the program; it exports nothing. So the app is taken from the last http.createServer call made while it loads
// (express-ws makes one per router and one for the app). Then the two set-up steps startServer runs before listening are run here.
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
