import { send } from '../http.js';

// Signs in through an upstream that sets two cookies, a session and a CSRF token, as an auth proxy or an OAuth flow gets them. With only
// one of them, the sign-in failed. The upstream is at KERN_TEST_AUTH_URL.
export function createApp() {
  return async (req, res) => {
    const answer = await fetch(`${process.env.KERN_TEST_AUTH_URL}/session`);
    await answer.arrayBuffer();
    const cookies = answer.headers.getSetCookie();
    if (cookies.length < 2) return send(res, 502, { error: 'the sign-in set too few cookies', cookies: cookies.length });
    return send(res, 200, { cookies: cookies.length });
  };
}
