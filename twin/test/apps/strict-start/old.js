import { send } from '../http.js';

// Reads its settings from a config service while it starts, and ends its own process when it cannot, as many apps do.
export async function createApp() {
  let settings;
  try {
    settings = await (await fetch(`${process.env.KERN_TEST_CONFIG_URL}/settings`)).json();
  } catch (error) {
    console.error('cannot read the settings', error);
    process.exit(1);
  }
  return (req, res) => send(res, 200, { greeting: settings.greeting });
}
