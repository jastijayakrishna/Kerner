import http from 'node:http';

// A stand-in for the two Supabase reads firecrawl's credit check makes, so the lines pull request #2638 changed can run without
// firecrawl's cloud:
// - GET /rest/v1/rpc/auth_credit_usage_chunk_38: an API key's credit row, with the fields of firecrawl's own mockACUC
//   (src/controllers/auth.ts);
// - GET /rest/v1/teams: a team's auto-recharge settings (off).
// Anything else gets an empty success, so nothing waits on it. The one key belongs to a team that has used 100 of its 1,000 credits:
// 10%, under every notification threshold.
// The same key firecrawl sends as input_key (a valid v4 UUID without dashes; see scenario.mjs).
const KEY = '00000000000040008000000000000001';
const row = (isExtract) => ({
  api_key: KEY, api_key_id: 1, team_id: 'proof-team', sub_id: 'proof-sub',
  sub_current_period_start: '2026-10-01T00:00:00.000Z', sub_current_period_end: '2026-10-31T00:00:00.000Z',
  sub_user_id: 'proof-user', price_id: 'proof-price',
  rate_limits: { crawl: 99999999, scrape: 99999999, extract: 99999999, search: 99999999, map: 99999999, preview: 99999999, crawlStatus: 99999999, extractStatus: 99999999, extractAgentPreview: 99999999, scrapeAgentPreview: 99999999 },
  price_credits: 1000, price_should_be_graceful: false, price_associated_auto_recharge_price_id: null,
  credits_used: 100, coupon_credits: 0, adjusted_credits_used: 100, remaining_credits: 900, total_credits_sum: 1000,
  plan_priority: { bucketLimit: 25, planModifier: 0.1 }, concurrency: 99999999, flags: null, is_extract: isExtract,
});

http.createServer((req, res) => {
  const url = new URL(req.url, 'http://standin');
  let body = [];
  if (url.pathname === '/rest/v1/rpc/auth_credit_usage_chunk_38') {
    const key = String(url.searchParams.get('input_key') ?? '').replace(/^fc-/, '').replaceAll('-', '');
    body = key === KEY ? [row(url.searchParams.get('i_is_extract') === 'true')] : [];
  } else if (url.pathname === '/rest/v1/teams') {
    body = { auto_recharge: false, auto_recharge_threshold: 1000 };
  }
  req.resume();
  req.on('end', () => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  });
}).listen(54321, '0.0.0.0', () => console.log('supabase stand-in listening on :54321'));
