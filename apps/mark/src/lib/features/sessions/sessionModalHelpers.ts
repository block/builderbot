export interface ParsedToolCall {
  name: string;
  args: Record<string, unknown>;
}

export function hasXmlBlocks(content: string): boolean {
  return /<(action|branch-history)>/.test(content);
}

export function parseToolCall(content: string): ParsedToolCall | null {
  try {
    const parsed = JSON.parse(content);
    if (parsed.name) {
      return {
        name: parsed.name,
        args: parsed.arguments || parsed.args || parsed.input || {},
      };
    }
  } catch {
    // not JSON
  }
  return null;
}

export function formatToolName(content: string): string {
  const parsed = parseToolCall(content);
  if (parsed) return parsed.name;
  return content;
}

export function formatToolArgs(content: string): string {
  const parsed = parseToolCall(content);
  if (!parsed || !parsed.args) return '';
  const entries = Object.entries(parsed.args);
  if (entries.length === 0) return '';
  return entries
    .map(([key, value]) => {
      const v = typeof value === 'string' ? value : JSON.stringify(value);
      return `${key}: ${v}`;
    })
    .join(', ');
}
