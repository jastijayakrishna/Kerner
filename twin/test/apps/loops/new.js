import { setInterval as every, scheduler } from 'node:timers/promises';
import { send } from '../http.js';




export function createApp() {
  const rounds = { interval: 0, wait: 0, yield: 0 };
  (async () => {
    for await (const _ of every(5)) rounds.interval++;
  })();
  (async () => {
    for (;;) {
      await scheduler.wait(5);
      rounds.wait++;
    }
  })();
  (async () => {
    for (;;) {
      await scheduler.yield();
      rounds.yield++;
    }
  })();
  return async (req, res) => {
    for (const name of Object.keys(rounds)) rounds[name] = 0;
    let own = 0;
    for await (const _ of every(10)) if (++own === 3) break;
    await scheduler.wait(20);
    await scheduler.yield();
    send(res, 200, Object.fromEntries(Object.entries(rounds).map(([name, n]) => [name, n > 0])));
  };
}
