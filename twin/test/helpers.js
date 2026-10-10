import { fileURLToPath } from 'node:url';

/** One version of a test app, as the twin loads it: an absolute file and the function that builds the request listener. */
export const app = (name, version) => ({ module: fileURLToPath(new URL(`./apps/${name}/${version}.js`, import.meta.url)), export: 'createApp' });
