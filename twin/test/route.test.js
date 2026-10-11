import { test } from 'node:test';
import assert from 'node:assert/strict';
import { routeOf } from '../src/route.js';

test('a route is the method and the path, with what looks like an id replaced', () => {
  assert.equal(routeOf('get', '/users/42/orders/9f1c2b7e-1d2a-4b5c-8d9e-0a1b2c3d4e5f?expand=1'), 'GET /users/:id/orders/:id');
  assert.equal(routeOf('POST', '/carts/5f2b9c0e8a1d4e7f9b3c2a10/items'), 'POST /carts/:id/items');
  assert.equal(routeOf('GET', '/s/AbCdEfGhIjKlMnOpQrStUv'), 'GET /s/:id', 'a long token is an id');
  assert.equal(routeOf('GET', '/files/report-final'), 'GET /files/report-final', 'a short name stays');
  assert.equal(routeOf('GET', '/'), 'GET /');
  assert.equal(routeOf('GET', '/#top'), 'GET /');
});
