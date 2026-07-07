import { describe, expect, it } from 'vitest';
import type { SessionMessage } from '../../types';
import {
  buildAcpTranscriptGroups,
  groupRichToolsByVerb,
  latestAvailableCommands,
  type RichToolItem,
} from './acpTranscript';

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

describe('buildAcpTranscriptGroups', () => {
  it('pairs tool results by ACP tool call id without relying on adjacency', () => {
    const visible = [
      message({ id: 1, role: 'user', content: 'go' }),
      message({
        id: 2,
        role: 'tool_call',
        content: JSON.stringify({ name: 'Read', input: { file_path: '/repo/src/main.rs' } }),
        acpEventKind: 'tool_call',
        acpToolCallId: 'tc-1',
      }),
      message({ id: 3, role: 'assistant', content: 'working' }),
      message({
        id: 4,
        role: 'tool_result',
        content: 'done',
        acpToolCallId: 'tc-1',
      }),
    ];

    const groups = buildAcpTranscriptGroups(visible, [], '/repo');
    expect(groups.map((group) => group.type)).toEqual(['user', 'tools', 'assistant']);
    const toolGroup = groups[1];
    expect(toolGroup.type).toBe('tools');
    if (toolGroup.type === 'tools') {
      expect(toolGroup.items[0].result?.id).toBe(4);
      expect(toolGroup.items[0].detail).toBe('src/main.rs');
    }
  });

  it('merges latest ACP tool status and raw output from metadata rows', () => {
    const visible = [
      message({
        id: 1,
        role: 'tool_call',
        content: 'Run tests',
        acpEventKind: 'tool_call',
        acpToolCallId: 'tc-2',
        acpToolStatus: 'pending',
        acpRawInput: { command: 'npm test' },
      }),
    ];
    const metadata = [
      message({
        id: 2,
        role: 'assistant',
        acpEventKind: 'tool_call_update',
        acpToolCallId: 'tc-2',
        acpToolStatus: 'failed',
        acpRawOutput: { exitCode: 1 },
      }),
    ];

    const groups = buildAcpTranscriptGroups(visible, metadata);
    const toolGroup = groups[0];
    expect(toolGroup.type).toBe('tools');
    if (toolGroup.type === 'tools') {
      expect(toolGroup.items[0].status).toBe('failed');
      expect(toolGroup.items[0].rawOutput).toEqual({ exitCode: 1 });
    }
  });

  it('does not surface permission metadata as transcript events', () => {
    const metadata = [
      message({
        id: 1,
        role: 'assistant',
        acpEventKind: 'permission_request',
        acpContent: {
          requestId: 'perm-1',
          status: 'pending',
          options: [{ optionId: 'allow-once', name: 'Allow once', kind: 'allow_once' }],
        },
      }),
      message({
        id: 2,
        role: 'assistant',
        acpEventKind: 'permission_response',
        acpContent: {
          requestId: 'perm-1',
          status: 'selected',
          selectedOptionId: 'allow-once',
        },
      }),
    ];

    const groups = buildAcpTranscriptGroups([], metadata);
    expect(groups).toEqual([]);
  });

  it('hides operational ACP metadata rows from the transcript', () => {
    const metadata = [
      message({
        id: 1,
        role: 'assistant',
        acpEventKind: 'usage_update',
        acpUsage: { inputTokens: 10, outputTokens: 5 },
      }),
      message({
        id: 2,
        role: 'assistant',
        acpEventKind: 'prompt_response',
        acpUsage: { inputTokens: 20, outputTokens: 8 },
      }),
      message({
        id: 3,
        role: 'assistant',
        acpEventKind: 'config_options_update',
        acpConfigOptions: [{ name: 'Model' }],
      }),
      message({
        id: 4,
        role: 'assistant',
        acpEventKind: 'session_mode_state',
        acpSessionModeState: { currentModeId: 'default' },
      }),
      message({
        id: 5,
        role: 'assistant',
        acpEventKind: 'current_mode_update',
        acpContent: { currentModeId: 'default' },
      }),
      message({
        id: 6,
        role: 'assistant',
        acpEventKind: 'available_commands_update',
        acpContent: { availableCommands: [{ name: 'plan' }] },
      }),
    ];

    const groups = buildAcpTranscriptGroups([], metadata);
    expect(groups).toEqual([]);
  });

  it('continues surfacing plan metadata rows', () => {
    const groups = buildAcpTranscriptGroups(
      [],
      [
        message({
          id: 1,
          role: 'assistant',
          acpEventKind: 'plan_update',
          acpContent: { entries: [{ content: 'Check UI', status: 'pending' }] },
        }),
      ]
    );

    expect(groups).toHaveLength(1);
    expect(groups[0].type).toBe('acp');
  });

  it('marks generate_pikchr tools and extracts the inner session id', () => {
    const groups = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'generate_pikchr',
            input: { description: 'Show the signup flow' },
          }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-pikchr',
        }),
      ],
      [
        message({
          id: 2,
          role: 'assistant',
          acpEventKind: 'tool_call_update',
          acpToolCallId: 'tc-pikchr',
          acpToolStatus: 'completed',
          acpRawOutput: {
            structuredContent: {
              innerSessionId: 'child-session-1',
              previewImagePath: '/tmp/preview.png',
            },
          },
        }),
      ]
    );

    expect(groups[0].type).toBe('tools');
    if (groups[0].type === 'tools') {
      expect(groups[0].items[0].isPikchrDiagramTool).toBe(true);
      expect(groups[0].items[0].innerSessionId).toBe('child-session-1');
    }
  });

  it('extracts Pikchr inner session ids from nested snake-case structured output', () => {
    const groups = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'mcp.generate_pikchr',
            input: { description: 'Show the signup flow' },
          }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-pikchr',
        }),
      ],
      [
        message({
          id: 2,
          role: 'assistant',
          acpEventKind: 'tool_call_update',
          acpToolCallId: 'tc-pikchr',
          acpRawOutput: {
            result: {
              structured_content: {
                inner_session_id: 'child-session-2',
              },
            },
          },
        }),
      ]
    );

    expect(groups[0].type).toBe('tools');
    if (groups[0].type === 'tools') {
      expect(groups[0].items[0].innerSessionId).toBe('child-session-2');
    }
  });

  it('recognizes delimiter-qualified generate_pikchr tool names', () => {
    const groups = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'pikchr_generate_pikchr',
            input: { description: 'Show the signup flow' },
          }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-pikchr-single-underscore',
        }),
        message({
          id: 2,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'mcp__pikchr__generate_pikchr',
            input: { description: 'Show the signup flow' },
          }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-pikchr-double-underscore',
        }),
      ],
      []
    );

    expect(groups[0].type).toBe('tools');
    if (groups[0].type === 'tools') {
      expect(groups[0].items.map((item) => item.isPikchrDiagramTool)).toEqual([true, true]);
    }
  });
});

describe('groupRichToolsByVerb', () => {
  it('collapses adjacent tools with the same verb', () => {
    const groups = groupRichToolsByVerb([
      richTool({ key: 'tool:1', verb: 'Read', detail: 'src/a.ts' }),
      richTool({ key: 'tool:2', verb: 'Read', detail: 'src/b.ts' }),
    ]);

    expect(groups).toHaveLength(1);
    expect(groups[0].verb).toBe('Read');
    expect(groups[0].summary).toBe('2 files');
    expect(groups[0].items.map((item) => item.detail)).toEqual(['src/a.ts', 'src/b.ts']);
  });

  it('keeps different adjacent verbs separate', () => {
    const groups = groupRichToolsByVerb([
      richTool({ key: 'tool:1', verb: 'Read' }),
      richTool({ key: 'tool:2', verb: 'Searched' }),
      richTool({ key: 'tool:3', verb: 'Read' }),
    ]);

    expect(groups.map((group) => group.verb)).toEqual(['Read', 'Searched', 'Read']);
    expect(groups.map((group) => group.items)).toHaveLength(3);
  });

  it('does not merge same verbs across transcript group boundaries', () => {
    const transcript = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({ name: 'Read', input: { file_path: '/repo/a.ts' } }),
        }),
        message({ id: 2, role: 'tool_result', content: 'a' }),
        message({ id: 3, role: 'assistant', content: 'middle' }),
        message({
          id: 4,
          role: 'tool_call',
          content: JSON.stringify({ name: 'Read', input: { file_path: '/repo/b.ts' } }),
        }),
        message({ id: 5, role: 'tool_result', content: 'b' }),
      ],
      [],
      '/repo'
    );
    const toolGroups = transcript.filter((group) => group.type === 'tools');

    expect(transcript.map((group) => group.type)).toEqual(['tools', 'assistant', 'tools']);
    expect(toolGroups).toHaveLength(2);
    for (const group of toolGroups) {
      if (group.type === 'tools') {
        expect(groupRichToolsByVerb(group.items)).toHaveLength(1);
        expect(group.items).toHaveLength(1);
      }
    }
  });

  it('keeps ACP status and structured metadata on grouped items', () => {
    const transcript = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({ name: 'Read', input: { file_path: '/repo/a.ts' } }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-1',
          acpToolStatus: 'completed',
        }),
        message({
          id: 2,
          role: 'tool_call',
          content: JSON.stringify({ name: 'Read', input: { file_path: '/repo/b.ts' } }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-2',
        }),
      ],
      [
        message({
          id: 3,
          role: 'assistant',
          acpEventKind: 'tool_call_update',
          acpToolCallId: 'tc-2',
          acpToolStatus: 'failed',
          acpRawInput: { file_path: '/repo/b.ts' },
          acpRawOutput: { error: 'missing' },
          acpContent: [{ type: 'content', content: { type: 'text', text: 'not found' } }],
          acpLocations: [{ path: '/repo/b.ts', line: 12 }],
        }),
      ],
      '/repo'
    );
    const toolGroup = transcript.find((group) => group.type === 'tools');

    expect(toolGroup?.type).toBe('tools');
    if (toolGroup?.type === 'tools') {
      const groups = groupRichToolsByVerb(toolGroup.items);
      expect(groups).toHaveLength(1);
      expect(groups[0].statusTone).toBe('danger');
      expect(groups[0].items[1].status).toBe('failed');
      expect(groups[0].items[1].rawInput).toEqual({ file_path: '/repo/b.ts' });
      expect(groups[0].items[1].rawOutput).toEqual({ error: 'missing' });
      expect(groups[0].items[1].content).toEqual([
        { type: 'content', content: { type: 'text', text: 'not found' } },
      ]);
      expect(groups[0].items[1].locations).toEqual([{ path: '/repo/b.ts', line: 12 }]);
    }
  });

  it('keeps Pikchr tools out of generic verb groups', () => {
    const groups = groupRichToolsByVerb([
      richTool({ key: 'tool:1', verb: 'Ran', detail: 'npm test' }),
      richTool({
        key: 'tool:pikchr',
        verb: 'Ran',
        detail: 'generate_pikchr',
        isPikchrDiagramTool: true,
        innerSessionId: 'child-session-1',
      }),
      richTool({ key: 'tool:2', verb: 'Ran', detail: 'npm build' }),
    ]);

    expect(groups).toHaveLength(3);
    expect(groups.map((group) => group.items.map((item) => item.key))).toEqual([
      ['tool:1'],
      ['tool:pikchr'],
      ['tool:2'],
    ]);
  });
});

describe('latestAvailableCommands', () => {
  it('extracts slash commands from the latest ACP command update', () => {
    const commands = latestAvailableCommands([
      message({
        id: 1,
        role: 'assistant',
        acpEventKind: 'available_commands_update',
        acpContent: {
          availableCommands: [
            {
              name: 'plan',
              description: 'Create a plan',
              input: { hint: 'goal' },
            },
          ],
        },
      }),
    ]);

    expect(commands).toEqual([
      {
        name: 'plan',
        description: 'Create a plan',
        inputHint: 'goal',
      },
    ]);
  });
});

function richTool(
  overrides: Partial<RichToolItem> & Pick<RichToolItem, 'key' | 'verb'>
): RichToolItem {
  return {
    call: message({ id: Number(overrides.key.replace(/\D/g, '')) || 1, role: 'tool_call' }),
    result: null,
    detail: '',
    status: 'completed',
    statusLabel: 'Succeeded',
    statusTone: 'success',
    toolCallId: null,
    toolKind: null,
    rawInput: undefined,
    rawOutput: undefined,
    content: undefined,
    locations: undefined,
    isPikchrDiagramTool: false,
    innerSessionId: null,
    ...overrides,
  };
}
