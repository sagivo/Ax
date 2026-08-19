const http = require("http");

http.createServer((_req, res) => {
  const body = JSON.stringify({ ok: true });
  res.writeHead(200, {
    "Content-Type": "application/json",
    "Content-Length": Buffer.byteLength(body),
  });
  res.end(body);
}).listen(18080, "127.0.0.1");
