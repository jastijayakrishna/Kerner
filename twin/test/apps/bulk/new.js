import { send } from '../http.js';
import { writeRow } from './old.js';


export function createApp() {
  return async (req, res) => {
    for (let row = 0; row < 300; row++) await writeRow(row);
    await writeRow(299);
    send(res, 200, { imported: 300 });
  };
}
