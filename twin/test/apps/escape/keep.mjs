import fs from 'node:fs';

// Loaded with --import before a ghost's seal goes on: it keeps the real fs.writeFileSync where the app can find it.
globalThis.__kernRealWrite = fs.writeFileSync;
