import { send } from '../http.js';

// Gathers ten reports from a data service (KERN_TEST_DATA) for one request: each answer is small enough to copy, all of them together
// are not.
export function createApp() {
  return async (req, res) => {
    let size = 0;
    for (let i = 0; i < 10; i++) size += (await (await fetch(`${process.env.KERN_TEST_DATA}/report/${i}`)).arrayBuffer()).byteLength;
    send(res, 200, { size });
  };
}
