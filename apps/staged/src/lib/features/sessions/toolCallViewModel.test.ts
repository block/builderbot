import { describe, expect, it } from 'vitest';
import type { SessionMessage } from '../../types';
import type { RichToolItem, ToolStatus } from './acpTranscript';
import { buildToolCallViewModel } from './toolCallViewModel';

function message(
  overrides: Partial<SessionMessage> & Pick<SessionMessage, 'id' | 'role'>
): SessionMessage {
  return {
    sessionId: 'session-1',
    content: overrides.content ?? '',
    createdAt: overrides.id,
    ...overrides,
  };
}

function richTool(overrides: Partial<RichToolItem> = {}): RichToolItem {
  const status = overrides.status ?? 'completed';
  return {
    key: 'tool:tc-1',
    call: message({ id: 1, role: 'tool_call', content: 'Custom tool' }),
    result: null,
    verb: 'Ran',
    detail: '',
    status,
    statusLabel: statusLabel(status),
    statusTone: statusTone(status),
    toolCallId: 'tc-1',
    toolKind: null,
    rawInput: undefined,
    rawOutput: undefined,
    content: undefined,
    locations: undefined,
    isPikchrDiagramTool: false,
    innerSessionId: null,
    pikchrRenderSource: null,
    ...overrides,
  };
}

function statusLabel(status: ToolStatus): RichToolItem['statusLabel'] {
  switch (status) {
    case 'pending':
      return 'Pending';
    case 'in_progress':
      return 'In progress';
    case 'completed':
      return 'Succeeded';
    case 'failed':
      return 'Failed';
    case 'cancelled':
      return 'Cancelled';
  }
}

function statusTone(status: ToolStatus): RichToolItem['statusTone'] {
  switch (status) {
    case 'pending':
      return 'muted';
    case 'in_progress':
      return 'running';
    case 'completed':
      return 'success';
    case 'failed':
      return 'danger';
    case 'cancelled':
      return 'cancelled';
  }
}

describe('buildToolCallViewModel classification', () => {
  it('classifies from normalized toolKind before the visible verb', () => {
    const model = buildToolCallViewModel(
      richTool({
        toolKind: 'Edit',
        verb: 'Ran',
        rawInput: { path: '/repo/src/App.svelte' },
      }),
      '/repo'
    );

    expect(model.category).toBe('edit');
    expect(model.metadata.toolKind).toBe('edit');
    expect(model.metadata.targetPath).toBe('src/App.svelte');
  });

  it('classifies from parsed tool titles and extracts parsed input', () => {
    const model = buildToolCallViewModel(
      richTool({
        verb: 'Processed',
        call: message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'Read /repo/src/App.svelte',
            input: { file_path: '/repo/src/App.svelte' },
          }),
        }),
      }),
      '/repo'
    );

    expect(model.category).toBe('read');
    expect(model.metadata.toolName).toBe('Read /repo/src/App.svelte');
    expect(model.metadata.targetPath).toBe('src/App.svelte');
    expect(model.metadata.inputText).toContain('file_path');
  });

  it('classifies parsed_cmd metadata before falling back to display verbs', () => {
    const model = buildToolCallViewModel(
      richTool({
        verb: 'Ran',
        call: message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'Custom wrapper',
            input: {
              parsed_cmd: [
                {
                  type: 'search',
                  cmd: "rg -n 'needle' /repo/src",
                  query: 'needle',
                  path: '/repo/src',
                },
              ],
            },
          }),
        }),
      }),
      '/repo'
    );

    expect(model.category).toBe('search');
    expect(model.metadata.query).toBe('needle');
    expect(model.metadata.targetPath).toBe('src');
    expect(model.metadata.parsedCommands[0]).toMatchObject({
      type: 'search',
      cmd: "rg -n 'needle' /repo/src",
    });
  });

  it('falls back to ACP content blocks when tool identity is missing', () => {
    const model = buildToolCallViewModel(
      richTool({
        verb: 'Processed',
        content: [
          {
            type: 'diff',
            path: '/repo/src/a.ts',
            oldText: 'old line\n',
            newText: 'new line\n',
          },
        ],
      }),
      '/repo'
    );

    expect(model.category).toBe('edit');
    expect(model.metadata.diffs).toEqual([
      {
        path: 'src/a.ts',
        oldText: 'old line\n',
        newText: 'new line\n',
        kind: 'modified',
      },
    ]);
    expect(model.sections.some((section) => section.kind === 'diff')).toBe(true);
  });

  it('detects network-style unknown tools from raw input metadata', () => {
    const model = buildToolCallViewModel(
      richTool({
        verb: 'Processed',
        rawInput: { method: 'post', url: 'https://example.test/api' },
      })
    );

    expect(model.category).toBe('network');
    expect(model.metadata.method).toBe('POST');
    expect(model.metadata.url).toBe('https://example.test/api');
  });

  it('keeps unknown tools generic with raw JSON fallback sections', () => {
    const model = buildToolCallViewModel(
      richTool({
        verb: 'Processed',
        rawInput: { option: 'value' },
        rawOutput: { payload: { ok: true } },
      })
    );

    expect(model.category).toBe('generic');
    expect(model.output.state).toBe('output');
    expect(model.sections).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: 'input', text: expect.stringContaining('option') }),
        expect.objectContaining({ kind: 'raw_output', text: expect.stringContaining('payload') }),
      ])
    );
  });
});

describe('buildToolCallViewModel output handling', () => {
  it('suppresses raw output JSON when a structured error is shown', () => {
    const model = buildToolCallViewModel(
      richTool({
        status: 'failed',
        rawOutput: { error: 'permission denied', exitCode: 1 },
      })
    );

    expect(model.output.state).toBe('output');
    expect(model.output.errorText).toBe('permission denied');
    expect(model.output.exitCode).toBe(1);

    const errorIndex = model.sections.findIndex(
      (section) => section.kind === 'output' && section.label === 'Error'
    );
    expect(errorIndex).toBeGreaterThanOrEqual(0);
    expect(model.sections.some((section) => section.kind === 'raw_output')).toBe(false);
  });

  it('keeps structured stdout and stderr distinct for successful commands', () => {
    const model = buildToolCallViewModel(
      richTool({
        rawOutput: { stdout: 'tests passed', stderr: 'warning only', exitCode: 0 },
      })
    );

    expect(model.output.stdout).toBe('tests passed');
    expect(model.output.stderr).toBe('warning only');
    expect(model.output.errorText).toBe('');
    const outputLabels = model.sections.flatMap((section) =>
      section.kind === 'output' ? [section.label] : []
    );
    expect(outputLabels).toEqual(['Stdout', 'Stderr']);
    expect(model.sections.some((section) => section.kind === 'raw_output')).toBe(false);
  });

  it('falls back to legacy tool_result content when ACP output is absent', () => {
    const model = buildToolCallViewModel(
      richTool({
        result: message({
          id: 2,
          role: 'tool_result',
          content: '```text\nlegacy output\n```',
        }),
      })
    );

    expect(model.category).toBe('command');
    expect(model.output.primaryText).toBe('legacy output');
    expect(model.sections).toContainEqual({
      kind: 'output',
      label: 'Output',
      text: 'legacy output',
      tone: 'normal',
    });
  });

  it('shows a waiting state for pending tools without output', () => {
    const model = buildToolCallViewModel(
      richTool({
        status: 'pending',
        statusTone: 'muted',
        result: null,
      })
    );

    expect(model.output.state).toBe('waiting');
    expect(model.output.emptyLabel).toBe('Waiting for output');
    expect(model.sections).toContainEqual({ kind: 'empty', label: 'Waiting for output' });
  });

  it('shows a no-output state for completed tools without output', () => {
    const model = buildToolCallViewModel(
      richTool({
        status: 'completed',
        result: null,
      })
    );

    expect(model.output.state).toBe('empty');
    expect(model.output.emptyLabel).toBe('No output');
    expect(model.hasDetails).toBe(true);
    expect(model.sections).toContainEqual({ kind: 'empty', label: 'No output' });
  });
});
