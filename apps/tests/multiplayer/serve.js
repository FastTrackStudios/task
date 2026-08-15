// @ts-check
// Minimal static SPA server for the dx-built web bundle.
//
// `dx build` emits a fully static site (index.html + /wasm + /assets);
// we serve it with an index.html fallback so deep links like /vault
// work, exactly like `dx serve` would — but with zero rebuild/file-
// watch machinery, which removes the dx cold-build flake from the
// test run entirely (the bundle is built once, up front, by run.sh).
//
// Usage: node serve.js <publicDir> <port>

const http = require("http");
const fs = require("fs");
const path = require("path");

const publicDir = path.resolve(process.argv[2]);
const port = parseInt(process.argv[3], 10);

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".txt": "text/plain; charset=utf-8",
};

const server = http.createServer((req, res) => {
  let urlPath;
  try {
    urlPath = decodeURIComponent(new URL(req.url || "/", "http://x").pathname);
  } catch {
    res.writeHead(400).end();
    return;
  }
  let file = path.normalize(path.join(publicDir, urlPath));
  if (!file.startsWith(publicDir)) {
    res.writeHead(403).end();
    return;
  }
  let stat = fs.existsSync(file) ? fs.statSync(file) : null;
  if (stat?.isDirectory()) {
    file = path.join(file, "index.html");
    stat = fs.existsSync(file) ? fs.statSync(file) : null;
  }
  if (!stat) {
    // SPA fallback: every unknown route serves the app shell.
    file = path.join(publicDir, "index.html");
    if (!fs.existsSync(file)) {
      res.writeHead(404).end("no index.html");
      return;
    }
  }
  const ext = path.extname(file).toLowerCase();
  res.writeHead(200, {
    "content-type": MIME[ext] || "application/octet-stream",
    "cache-control": "no-store",
  });
  fs.createReadStream(file).pipe(res);
});

server.listen(port, "127.0.0.1", () => {
  // global-setup greps for this line to know we're up.
  console.log(`mp-static-server listening on http://127.0.0.1:${port}`);
});
