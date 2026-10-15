import { readJson, send } from '../http.js';


export function createApp() {
  return async (req, res) => {
    const policy = await readJson(req);
    const limit = policy.limits.max;
    send(res, 200, { region: policy.region ?? 'all', limit });
  };
}
