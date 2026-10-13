// Answers the JSON it was sent, byte for byte, without reading it: a body nested however deep comes back as it went.
export function createApp() {
  return async (req, res) => {
    const parts = [];
    for await (const part of req) parts.push(part);
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(Buffer.concat(parts));
  };
}
