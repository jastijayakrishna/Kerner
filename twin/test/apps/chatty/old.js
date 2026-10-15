import { send } from '../http.js';


export function createApp() {
  return async (req, res) => {
    let on = 0;
    for (let i = 0; i < 600; i++) on += (await (await fetch(`${process.env.KERN_TEST_DATA}/flag/${i}`)).text()) === '1' ? 1 : 0;
    send(res, 200, { on });
  };
}
