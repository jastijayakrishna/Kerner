import { send } from '../http.js';
import { writeRow } from './old.js';

// Imports the same 300 rows, then writes the last one again "to be safe": 301 writes where the old version made 300.
export function createApp() {
  return async (req, res) => {
    for (let row = 0; row < 300; row++) await writeRow(row);
    await writeRow(299);
    send(res, 200, { imported: 300 });
  };
}
