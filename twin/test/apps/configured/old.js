import { send } from '../http.js';

// Reads its settings from a config service while it starts, as real apps read their database or key store; then answers with them.
export async function createApp() {
  const settings = await (await fetch(`${process.env.KERN_TEST_CONFIG_URL}/settings`)).json();
  return (req, res) => send(res, 200, { greeting: settings.greeting });
}
