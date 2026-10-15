const KEY = 'fc-00000000000040008000000000000001';

export const scenario = {
  async step(client, state, random, i) {
    const pick = random();
    if (pick < 0.7) {
      await client.send('/v1/crawl', { method: 'POST', headers: { authorization: `Bearer ${KEY}`, 'content-type': 'application/json' }, body: JSON.stringify({ url: `https://example.com/page-${i}`, limit: 5000 }) });
    } else if (pick < 0.85) await client.send('/');
    else await client.send('/e2e-test');
  },
};
