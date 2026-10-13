import { send } from '../http.js';

// Asks a flag service (KERN_TEST_DATA) about 600 features, one call each, for one request: every call is tiny, there are many.
export function createApp() {
  return async (req, res) => {
    let on = 0;
    for (let i = 0; i < 600; i++) on += (await (await fetch(`${process.env.KERN_TEST_DATA}/flag/${i}`)).text()) === '1' ? 1 : 0;
    send(res, 200, { on });
  };
}
