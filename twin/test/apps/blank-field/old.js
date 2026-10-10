import { readJson, send } from '../http.js';

export function createApp() {
  return async (req, res) => {
    const policy = await readJson(req);
    send(res, 200, { region: policy.region ?? 'all', limit: policy.limits ? policy.limits.max : 100 });
  };
}
