import http from 'node:http';
import { send } from '../http.js';


const upstream = () => process.env.KERN_TEST_UPSTREAM;

export function getJson(path) {
  return new Promise((resolve, reject) => {
    http.get(new URL(path, upstream()), (res) => {
      const parts = [];
      res.on('data', (part) => parts.push(part));
      res.on('end', () => resolve(JSON.parse(Buffer.concat(parts).toString())));
    }).on('error', reject);
  });
}

export function postJson(path, value) {
  return new Promise((resolve, reject) => {
    const req = http.request(new URL(path, upstream()), { method: 'POST', headers: { 'content-type': 'application/json' } }, (res) => {
      res.resume();
      res.on('end', () => resolve(res.statusCode));
    });
    req.on('error', reject);
    req.end(JSON.stringify(value));
  });
}

export function createApp() {
  return async (req, res) => {
    if (req.url === '/rate') return send(res, 200, { rate: (await getJson('/rate')).rate });
    if (req.url === '/order') return send(res, 200, { charged: await postJson('/charge', { amount: 5 }) });
    return send(res, 404, { error: 'no such route' });
  };
}
