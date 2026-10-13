import { send } from '../http.js';

/** Writes one row to the upstream at KERN_TEST_ROWS_URL. */
export const writeRow = async (row) => (await fetch(process.env.KERN_TEST_ROWS_URL, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ row }) })).arrayBuffer();

// A bulk import: every request imports 300 rows, one write each.
export function createApp() {
  return async (req, res) => {
    for (let row = 0; row < 300; row++) await writeRow(row);
    send(res, 200, { imported: 300 });
  };
}
