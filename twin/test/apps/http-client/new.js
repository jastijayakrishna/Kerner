import { getJson, postJson } from './old.js';
import { send } from '../http.js';


export function createApp() {
  return async (req, res) => {
    if (req.url === '/rate') return send(res, 200, { rate: (await getJson('/rate')).rate });
    if (req.url === '/order') {
      await postJson('/charge', { amount: 5 });
      return send(res, 200, { charged: await postJson('/charge', { amount: 5 }) });
    }
    return send(res, 404, { error: 'no such route' });
  };
}
