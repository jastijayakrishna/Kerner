import { send } from '../http.js';

// The agent renamed user_name to username; older clients still read user_name.
export function createApp() {
  return (req, res) => send(res, 200, { username: 'ada', plan: 'pro' });
}
