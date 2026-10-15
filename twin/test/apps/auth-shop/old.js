import { send } from '../http.js';


export function createApp() {
  return (req, res) => {
    if (req.headers.authorization !== `Bearer ${process.env.KERN_TEST_TOKEN}`) return send(res, 401, { error: 'unauthorized' });
    return send(res, 200, { plan: 'pro' });
  };
}
