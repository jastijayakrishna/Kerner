
export function createApp() {
  return async (req, res) => {
    const parts = [];
    for await (const part of req) parts.push(part);
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(Buffer.concat(parts));
  };
}
