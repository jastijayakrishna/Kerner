// firecrawl's traffic for the proof. Most requests are crawls asked for by a team that has used 100 of its 1,000 credits, each for more
// pages than it has credits left: every one is refused with 402 at the lines pull request #2638 changed. The rest are the API's two
// plain routes.
// A valid v4 UUID (version nibble 4, variant 8): firecrawl's parseApi reads the key as a UUID and rejects anything else with 401 before
// it ever looks the key up, so an all-zeros key never reaches the credit check.
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
