import { test } from 'node:test';
import assert from 'node:assert/strict';
import { app, listen, routed, until } from './helpers.js';

async function configService() {
  const asked = [];
  const service = await listen((req, res) => {
    asked.push(req.url);
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(req.url === '/flags' ? '{"beta":true}' : '{"greeting":"hello"}');
  });
  process.env.KERN_TEST_CONFIG_URL = service.url;
  return {
    asked,
    close: async () => {
      delete process.env.KERN_TEST_CONFIG_URL;
      await service.close();
    },
  };
}

test('without a record of its start, a version that reads while it starts cannot load in a ghost, and the twin says what to do', async () => {
  const config = await configService();
  try {
    const shop = await routed('configured');
    await until(() => shop.twin.stats().disabled);
    assert.match(shop.twin.stats().disabled, /could not start: the old version .*fetch failed \(at its start it made calls the twin has no answer for: GET http:\/\/127\.0\.0\.1:\d+\/settings; with load: 'record'/);
    assert.deepEqual(await (await fetch(shop.url)).json(), { greeting: 'hello' }, 'customers are served as always');
    await shop.close();
  } finally {
    await config.close();
  }
});

test('with load: "record", the ghosts start from the record of the app\'s own start, and only the app asks the config service', async () => {
  const config = await configService();
  try {
    const shop = await routed('configured', { load: 'record' });
    for (let i = 0; i < 3; i++) assert.deepEqual(await (await fetch(shop.url)).json(), { greeting: 'hello' });
    await shop.twin.idle();
    const stats = shop.twin.stats();
    assert.deepEqual([stats.disabled, stats.same, stats.notCompared], [null, 3, 0], JSON.stringify(stats));
    assert.deepEqual(config.asked, ['/settings'], 'the real start asked once; both ghosts were answered from its record');
    await shop.close();
  } finally {
    await config.close();
  }
});

test('a start the seal refuses names what it refused, and how to allow a native add-on that only computes', async () => {
  const shop = await routed('native');
  await until(() => shop.twin.stats().disabled);
  assert.match(shop.twin.stats().disabled, /could not start: the old version .*refused it: loading the native add-on \/opt\/shop\/node_modules\/fast-hash\/build\/Release\/fast_hash\.node; .*addons: \["fast-hash"\]/);
  assert.deepEqual(await (await fetch(shop.url)).json(), { ok: true }, 'customers are served as always');
  await shop.close();
});

test('a native add-on loaded from a path outside node_modules is named, and allowed, by its file name', async () => {
  const shop = await routed('native-path');
  await until(() => shop.twin.stats().disabled);
  assert.match(shop.twin.stats().disabled, /refused it: loading the native add-on \/opt\/shop\/native\/fast_hash\.linux-x64-gnu\.node; .*addons: \["fast_hash"\]/, shop.twin.stats().disabled);
  await shop.close();
  const allowed = await routed('native-path', { addons: ['fast_hash'] });
  assert.deepEqual(await (await fetch(allowed.url)).json(), { ok: true });
  await allowed.twin.idle();
  assert.equal(allowed.twin.stats().disabled, null, JSON.stringify(allowed.twin.stats()));
  await allowed.close();
});

test('a start that ends its own process says so, with the calls the ghost could not answer, and leaves the app running', async () => {
  const config = await configService();
  try {
    const shop = await routed('strict-start');
    await until(() => shop.twin.stats().disabled);
    assert.match(shop.twin.stats().disabled, /could not start: the old version .*ended itself with process\.exit\(1\) while starting \(at its start it made calls the twin has no answer for: GET http:\/\/127\.0\.0\.1:\d+\/settings/);
    assert.deepEqual(await (await fetch(shop.url)).json(), { greeting: 'hello' }, 'customers are served as always');
    await shop.close();
  } finally {
    await config.close();
  }
});

test('a failed start names each call it could not answer once, with a count, so a repeated poll cannot hide the one that mattered', async () => {
  const config = await configService();
  try {
    const shop = await routed('polling-start');
    await until(() => shop.twin.stats().disabled);
    assert.match(shop.twin.stats().disabled, /no answer for: GET http:\/\/127\.0\.0\.1:\d+\/poll \(4 times\); GET http:\/\/127\.0\.0\.1:\d+\/settings;/);
    await shop.close();
  } finally {
    await config.close();
  }
});

test('a new version that asks at its start for what the old one never did cannot start, and the twin names the call', async () => {
  const config = await configService();
  try {
    const shop = await routed('configured', { load: 'record', candidate: app('configured', 'extra') });
    await until(() => shop.twin.stats().disabled);
    assert.match(shop.twin.stats().disabled, /the new version .*cannot answer: GET http:\/\/127\.0\.0\.1:\d+\/flags/);
    assert.deepEqual(await (await fetch(shop.url)).json(), { greeting: 'hello' }, 'customers are served as always');
    await shop.close();
  } finally {
    await config.close();
  }
});
