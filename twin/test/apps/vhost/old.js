import http from 'node:http';
import { send } from '../http.js';

// Reaches its upstream by address: by IP, naming the site in a Host header of its own (a virtual host; a signed request names its host
// the same way), and by an IPv6 address. The upstream's ports are in KERN_TEST_UPSTREAM_PORT and KERN_TEST_UPSTREAM6_PORT.
function getJson(options) {
  return new Promise((resolve, reject) => {
    http.get(options, (res) => {
      const parts = [];
      res.on('data', (part) => parts.push(part));
      res.on('end', () => resolve(JSON.parse(Buffer.concat(parts).toString())));
    }).on('error', reject);
  });
}

export function createApp() {
  return async (req, res) => {
    if (req.url === '/vhost') {
      return send(res, 200, await getJson({ host: '127.0.0.1', port: Number(process.env.KERN_TEST_UPSTREAM_PORT), path: '/rate', headers: { Host: 'rates.example' } }));
    }
    if (req.url === '/ipv6') return send(res, 200, await getJson({ host: '::1', port: Number(process.env.KERN_TEST_UPSTREAM6_PORT), path: '/rate' }));
    return send(res, 404, { error: 'no such route' });
  };
}
