import { send } from '../http.js';


export async function createApp() {
  const base = process.env.KERN_TEST_CONFIG_URL;
  for (let i = 0; i < 4; i++) await fetch(`${base}/poll`).catch(() => {});
  const settings = await (await fetch(`${base}/settings`)).json();
  return (req, res) => send(res, 200, { greeting: settings.greeting });
}
