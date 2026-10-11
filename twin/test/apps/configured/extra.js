import { send } from '../http.js';

// A new version that also reads feature flags while it starts: a call the old version's start never made.
export async function createApp() {
  const base = process.env.KERN_TEST_CONFIG_URL;
  const settings = await (await fetch(`${base}/settings`)).json();
  const flags = await (await fetch(`${base}/flags`)).json();
  return (req, res) => send(res, 200, { greeting: settings.greeting, beta: flags.beta });
}
