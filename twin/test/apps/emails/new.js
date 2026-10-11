import { readJson, send } from '../http.js';

// Treats one company's own domain differently: a difference that depends on the private value itself.
export function createApp() {
  return async (req, res) => {
    const { email } = await readJson(req);
    send(res, 200, { domain: email.endsWith('@corp.com') ? 'internal' : email.split('@')[1] });
  };
}
