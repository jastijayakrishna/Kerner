import { send } from '../http.js';


export function createApp() {
  return (req, res) => {
    if (req.headers.authorization && req.headers.authorization !== 'Bearer admin-token') return send(res, 401, { error: 'sign in first' });
    return send(res, 200, { users: 3 });
  };
}
