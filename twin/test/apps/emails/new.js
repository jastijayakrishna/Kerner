import { readJson, send } from '../http.js';


export function createApp() {
  return async (req, res) => {
    const { email } = await readJson(req);
    send(res, 200, { domain: email.endsWith('@corp.com') ? 'internal' : email.split('@')[1] });
  };
}
