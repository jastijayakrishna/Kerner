import { send } from '../http.js';

// The new version names the plan differently, and words its refusal differently too.
export function createApp() {
  return (req, res) => {
    if (req.headers.authorization !== `Bearer ${process.env.KERN_TEST_TOKEN}`) return send(res, 401, { error: 'not signed in' });
    return send(res, 200, { plan: 'premium' });
  };
}
