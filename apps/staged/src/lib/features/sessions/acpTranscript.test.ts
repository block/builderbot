import { describe, expect, it } from 'vitest';
import type { SessionMessage } from '../../types';
import { buildAcpTranscriptGroups, latestAvailableCommands } from './acpTranscript';

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
