/**
 * Minimal MCP Streamable HTTP client for e2e tests.
 * Speaks JSON-RPC 2.0 over HTTP to the penpal MCP endpoint.
 * Handles both plain JSON and SSE response formats.
 */

const MCP_URL = 'http://localhost:18923/mcp';

/**
 * Parse an SSE response body and return the JSON-RPC result from the
 * first `event: message` data line.
 */
function parseSSE(text: string): unknown {
  for (const block of text.split('\n\n')) {
    const lines = block.split('\n');
    const dataLine = lines.find((l) => l.startsWith('data: '));
    if (dataLine) {
      return JSON.parse(dataLine.slice(6));
    }
  }
  throw new Error(`No data found in SSE response: ${text.slice(0, 200)}`);
}

async function parseResponse(res: Response): Promise<unknown> {
  const ct = res.headers.get('content-type') || '';
  if (ct.includes('text/event-stream')) {
    return parseSSE(await res.text());
  }
  return res.json();
}

export class MCPClient {
  private sessionId: string | null = null;
  private nextId = 1;

  async initialize(): Promise<void> {
    const res = await fetch(MCP_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: this.nextId++,
        method: 'initialize',
        params: {
          protocolVersion: '2025-03-26',
          capabilities: {},
          clientInfo: { name: 'e2e-test', version: '1.0.0' },
        },
      }),
    });

    const sid = res.headers.get('mcp-session-id');
    if (sid) this.sessionId = sid;

    // Read and discard the initialize response
    await parseResponse(res);

    // Send initialized notification
    await fetch(MCP_URL, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(this.sessionId ? { 'mcp-session-id': this.sessionId } : {}),
      },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: 'notifications/initialized',
      }),
    });
  }

  async callTool(name: string, args: Record<string, unknown>): Promise<unknown> {
    const res = await fetch(MCP_URL, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(this.sessionId ? { 'mcp-session-id': this.sessionId } : {}),
      },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: this.nextId++,
        method: 'tools/call',
        params: { name, arguments: args },
      }),
    });

    const json = (await parseResponse(res)) as Record<string, unknown>;
    if (json.error) {
      throw new Error(`MCP error: ${JSON.stringify(json.error)}`);
    }
    return json.result;
  }
}
