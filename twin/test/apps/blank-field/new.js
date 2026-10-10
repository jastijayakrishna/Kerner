import { readJson, send } from '../http.js';

// A new quota check that reads limits.max without checking that limits is there: it only breaks on policies with blank limits.
export function createApp() {
  return async (req, res) => {
    const policy = await readJson(req);
    const limit = policy.limits.max;
    send(res, 200, { region: policy.region ?? 'all', limit });
  };
}
