// Traffic for the three-pull-request shop: limits (answered alike by both versions) and profiles (renamed by #102).
export const scenario = {
  async step(client, state, random, i) {
    const json = { 'content-type': 'application/json' };
    if (i % 2) await client.send('/profile', { method: 'POST', headers: json, body: JSON.stringify({ name: `n${i}`, plan: 'pro' }) });
    else await client.send('/limit', { method: 'POST', headers: json, body: JSON.stringify({ limits: { max: 1 + Math.floor(random() * 9) } }) });
  },
};
