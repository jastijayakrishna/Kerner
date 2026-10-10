// Small helpers the test apps share: plain node:http, like a simple production app.
export async function readJson(req) {
  const parts = [];
  for await (const chunk of req) parts.push(chunk);
  const text = Buffer.concat(parts).toString('utf8');
  return text ? JSON.parse(text) : {};
}

export function send(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) });
  res.end(body);
}
