// The shop's traffic, as the Phase 4 acceptance run sends it: lists, lookups, orders (some large, which the new version rounds) and
// order lookups.
export const scenario = {
  async step(client, state, random) {
    const pick = random();
    if (pick < 0.4) await client.send('/products');
    else if (pick < 0.7) await client.send(`/products/${1 + Math.floor(random() * 12)}`);
    else if (pick < 0.95) {
      const quantity = (1 + Math.floor(random() * 3)) * (random() < 0.15 ? 6 : 1);
      const answer = await client.send('/orders', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ productId: 1 + Math.floor(random() * 10), quantity }) });
      if (answer.status === 201) (state.orders ??= []).push(answer.json().id);
    } else if (state.orders?.length) await client.send(`/orders/${state.orders[Math.floor(random() * state.orders.length)]}`);
    else await client.send('/products');
  },
};
