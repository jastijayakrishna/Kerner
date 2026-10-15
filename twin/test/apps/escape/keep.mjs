import fs from 'node:fs';


globalThis.__kernRealWrite = fs.writeFileSync;
