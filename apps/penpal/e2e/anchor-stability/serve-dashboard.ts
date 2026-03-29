/**
 * Minimal HTTP server for the anchor stability dashboard.
 * Serves dashboard.html at / and results from the results/ directory.
 *
 * Usage: npx tsx e2e/anchor-stability/serve-dashboard.ts
 */

import * as http from 'http';
import * as fs from 'fs';
import * as path from 'path';

const PORT = parseInt(process.env.DASHBOARD_PORT ?? '18950', 10);
const BASE_DIR = path.resolve(__dirname, 'results');
const DASHBOARD_HTML = path.resolve(__dirname, 'dashboard.html');

const MIME: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
};

const server = http.createServer((req, res) => {
  const url = new URL(req.url ?? '/', `http://localhost:${PORT}`);
  let filePath: string;

  if (url.pathname === '/' || url.pathname === '/dashboard') {
    filePath = DASHBOARD_HTML;
  } else {
    // Serve from results directory, stripping leading /
    const rel = url.pathname.replace(/^\/+/, '');
    filePath = path.join(BASE_DIR, rel);

    // Security: prevent path traversal
    if (!filePath.startsWith(BASE_DIR)) {
      res.writeHead(403);
      res.end('Forbidden');
      return;
    }
  }

  const ext = path.extname(filePath);
  res.setHeader('Content-Type', MIME[ext] || 'application/octet-stream');
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader('Cache-Control', 'no-cache');

  try {
    const data = fs.readFileSync(filePath);
    res.writeHead(200);
    res.end(data);
  } catch {
    res.writeHead(404);
    res.end('Not found');
  }
});

server.listen(PORT, () => {
  console.log(`Dashboard: http://localhost:${PORT}`);
});

// Graceful shutdown
process.on('SIGINT', () => {
  server.close();
  process.exit(0);
});
process.on('SIGTERM', () => {
  server.close();
  process.exit(0);
});
