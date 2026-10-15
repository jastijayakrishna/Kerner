import { send } from '../http.js';


export async function createApp() {
  const settings = await (await fetch(`${process.env.KERN_TEST_CONFIG_URL}/settings`)).json();
  return (req, res) => send(res, 200, { greeting: settings.greeting });
}
