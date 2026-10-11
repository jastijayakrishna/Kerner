// A small shop with three routes; three pull requests each change one of them.
function send(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(body);
}

function total(items) {
  return items.reduce((sum, item) => sum + item.price * item.count, 0);
}

function profile(user) {
  return { user_name: user.name, plan: user.plan };
}

function limit(policy) {
  return policy.limits ? policy.limits.max : 100;
}

export function createApp() {
  return async (req, res) => {
    const parts = [];
    for await (const part of req) parts.push(part);
    const input = parts.length ? JSON.parse(Buffer.concat(parts).toString()) : {};
    if (req.url === '/total') return send(res, 200, { total: total(input.items ?? []) });
    if (req.url === '/profile') return send(res, 200, profile(input));
    if (req.url === '/limit') return send(res, 200, { limit: limit(input) });
    return send(res, 404, { error: 'no such route' });
  };
}
