import { send } from '../http.js';

export function createApp() {
  return (req, res) => send(res, 200, { user_name: 'ada', plan: 'pro' });
}
