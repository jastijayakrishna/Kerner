import Redis from 'ioredis';

const send = (res, status, value) => {
  const body = JSON.stringify(value);
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(body);
};

export function createApp() {
  const redis = new Redis(process.env.KERN_TWIN_REDIS_URL);
  return async (req, res) => {
    if (req.url === '/hello') return send(res, 200, { greeting: await redis.get('greeting') });
    if (req.url === '/visit') {
      const visits = await redis.incr('visits');
      const [[, greeting], [, profile]] = await redis.pipeline().get('greeting').hgetall('profile').exec();
      return send(res, 200, { visits, greeting, plan: profile.plan ?? null });
    }
    return send(res, 404, { error: 'no such route' });
  };
}
