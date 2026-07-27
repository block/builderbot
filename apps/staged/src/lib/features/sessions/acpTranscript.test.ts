import { describe, expect, it } from 'vitest';
import type { SessionMessage } from '../../types';
import {
  buildAcpTranscriptGroups,
  groupRichToolsByVerb,
  isToolMetadataSettled,
  latestAvailableCommands,
  stabilizeAcpTranscriptGroups,
  toolHasDetails,
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

  it('links a running generate_pikchr tool to its announced child session', () => {
    const groups = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'mcp__pikchr__generate_pikchr',
            input: { description: 'Show the signup flow' },
          }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-pikchr',
          acpToolStatus: 'in_progress',
        }),
      ],
      [
        message({
          id: 2,
          role: 'assistant',
          acpEventKind: 'pikchr_session_started',
          acpContent: { innerSessionId: 'child-session-live' },
        }),
      ]
    );

    expect(groups).toHaveLength(1);
    expect(groups[0].type).toBe('tools');
    if (groups[0].type === 'tools') {
      expect(groups[0].items[0].status).toBe('in_progress');
      expect(groups[0].items[0].innerSessionId).toBe('child-session-live');
    }
  });

  it('prefers the tool output id and pairs remaining announcements in order', () => {
    const pikchrCall = (id: number, toolCallId: string) =>
      message({
        id,
        role: 'tool_call',
        content: JSON.stringify({
          name: 'generate_pikchr',
          input: { description: 'diagram' },
        }),
        acpEventKind: 'tool_call',
        acpToolCallId: toolCallId,
      });

    const groups = buildAcpTranscriptGroups(
      [pikchrCall(1, 'tc-done'), pikchrCall(4, 'tc-running')],
      [
        message({
          id: 2,
          role: 'assistant',
          acpEventKind: 'pikchr_session_started',
          acpContent: { innerSessionId: 'child-done' },
        }),
        message({
          id: 3,
          role: 'assistant',
          acpEventKind: 'tool_call_update',
          acpToolCallId: 'tc-done',
          acpToolStatus: 'completed',
          acpRawOutput: { structuredContent: { innerSessionId: 'child-done' } },
        }),
        message({
          id: 5,
          role: 'assistant',
          acpEventKind: 'pikchr_session_started',
          acpContent: { innerSessionId: 'child-running' },
        }),
      ]
    );

    const ids = groups.flatMap((group) =>
      group.type === 'tools' ? group.items.map((item) => item.innerSessionId) : []
    );
    expect(ids).toEqual(['child-done', 'child-running']);
  });

  it('does not let a stale unannounced pikchr tool steal a later announcement', () => {
    // A generate_pikchr call with neither an output-derived id nor its own
    // announcement (a pre-announcement transcript, or the backend's
    // announcement write failed) has no id source at all. A later call's
    // announcement must pair with the nearest preceding tool — its own call —
    // leaving the stale card unlinked rather than pointing it at the wrong
    // diagram session.
    const pikchrCall = (id: number, toolCallId: string) =>
      message({
        id,
        role: 'tool_call',
        content: JSON.stringify({
          name: 'generate_pikchr',
          input: { description: 'diagram' },
        }),
        acpEventKind: 'tool_call',
        acpToolCallId: toolCallId,
      });

    const groups = buildAcpTranscriptGroups(
      [pikchrCall(1, 'tc-stale'), pikchrCall(2, 'tc-new')],
      [
        message({
          id: 3,
          role: 'assistant',
          acpEventKind: 'pikchr_session_started',
          acpContent: { innerSessionId: 'child-new' },
        }),
      ]
    );

    const ids = groups.flatMap((group) =>
      group.type === 'tools' ? group.items.map((item) => item.innerSessionId) : []
    );
    expect(ids).toEqual([null, 'child-new']);
  });

  it('keeps failed pikchr tools linked to their announced child session', () => {
    // A failed call's result carries no structured content, so the
    // announcement is the only id source — dropping it would leave the
    // failure (recorded in the child session) unreachable from the chat.
    const groups = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'generate_pikchr',
            input: { description: 'diagram' },
          }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-pikchr',
          acpToolStatus: 'failed',
        }),
      ],
      [
        message({
          id: 2,
          role: 'assistant',
          acpEventKind: 'pikchr_session_started',
          acpContent: { innerSessionId: 'child-session-failed' },
        }),
      ]
    );

    expect(groups[0].type).toBe('tools');
    if (groups[0].type === 'tools') {
      expect(groups[0].items[0].status).toBe('failed');
      expect(groups[0].items[0].innerSessionId).toBe('child-session-failed');
    }
  });

  it('extracts the Pikchr source from a successful render_pikchr call', () => {
    const groups = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'mcp__pikchr-preview__render_pikchr',
            input: { pikchr: 'box "Clean" fit' },
          }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-render',
          acpRawInput: { pikchr: 'box "Clean" fit' },
        }),
      ],
      [
        message({
          id: 2,
          role: 'assistant',
          acpEventKind: 'tool_call_update',
          acpToolCallId: 'tc-render',
          acpToolStatus: 'completed',
        }),
      ]
    );

    expect(groups[0].type).toBe('tools');
    if (groups[0].type === 'tools') {
      expect(groups[0].items[0].pikchrRenderSource).toBe('box "Clean" fit');
      expect(groups[0].items[0].isPikchrDiagramTool).toBe(false);
    }
  });

  it('extracts the render_pikchr source from the call content when raw input is missing', () => {
    const groups = buildAcpTranscriptGroups(
      [
        message({
          id: 1,
          role: 'tool_call',
          content: JSON.stringify({
            name: 'pikchr_preview.render_pikchr',
            input: { pikchr: 'circle "Hub"' },
          }),
          acpEventKind: 'tool_call',
          acpToolCallId: 'tc-render',
          acpToolStatus: 'completed',
        }),
      ],
      []
    );

    expect(groups[0].type).toBe('tools');
    if (groups[0].type === 'tools') {
      expect(groups[0].items[0].pikchrRenderSource).toBe('circle "Hub"');
    }
  });

  it('keeps failed and still-running render_pikchr calls as plain tool cards', () => {
    const renderCall = (id: number, toolCallId: string, status: string) =>
      message({
        id,
        role: 'tool_call',
        content: JSON.stringify({
          name: 'mcp__pikchr-preview__render_pikchr',
          input: { pikchr: 'box "Broken" fit' },
        }),
        acpEventKind: 'tool_call',
        acpToolCallId: toolCallId,
        acpToolStatus: status,
        acpRawInput: { pikchr: 'box "Broken" fit' },
      });

    const groups = buildAcpTranscriptGroups(
      [renderCall(1, 'tc-failed', 'failed'), renderCall(2, 'tc-running', 'in_progress')],
      []
    );

    expect(groups[0].type).toBe('tools');
    if (groups[0].type === 'tools') {
      expect(groups[0].items.map((item) => item.pikchrRenderSource)).toEqual([null, null]);
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

  it('keeps inline-diagram render_pikchr tools out of generic verb groups', () => {
    const groups = groupRichToolsByVerb([
      richTool({
        key: 'tool:render-1',
        verb: 'Ran',
        detail: 'render_pikchr',
        pikchrRenderSource: 'box "First"',
      }),
      richTool({
        key: 'tool:render-2',
        verb: 'Ran',
        detail: 'render_pikchr',
        pikchrRenderSource: 'box "Second"',
      }),
      richTool({ key: 'tool:1', verb: 'Ran', detail: 'npm test' }),
    ]);

    expect(groups.map((group) => group.items.map((item) => item.key))).toEqual([
      ['tool:render-1'],
      ['tool:render-2'],
      ['tool:1'],
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

describe('stabilizeAcpTranscriptGroups', () => {
  const transcript = (visible: SessionMessage[], metadata: SessionMessage[]) =>
    buildAcpTranscriptGroups(visible, metadata, '/repo');

  it('returns the previous array identity when nothing changed', () => {
    const visible = [
      message({ id: 1, role: 'user', content: 'go' }),
      message({
        id: 2,
        role: 'tool_call',
        content: JSON.stringify({ name: 'Read', input: { file_path: '/repo/a.ts' } }),
        acpEventKind: 'tool_call',
        acpToolCallId: 'tc-1',
        acpToolStatus: 'completed',
      }),
      message({ id: 3, role: 'assistant', content: 'done' }),
    ];
    const metadata = [visible[1]];

    const first = transcript(visible, metadata);
    const second = stabilizeAcpTranscriptGroups(first, transcript(visible, metadata));

    expect(second).toBe(first);
  });

  it('reuses unchanged groups while replacing the changed tail', () => {
    const user = message({ id: 1, role: 'user', content: 'go' });
    const first = transcript([user, message({ id: 2, role: 'assistant', content: 'partial' })], []);
    const grown = transcript(
      [user, message({ id: 2, role: 'assistant', content: 'partial plus more' })],
      []
    );

    const stabilized = stabilizeAcpTranscriptGroups(first, grown);

    expect(stabilized).not.toBe(first);
    expect(stabilized[0]).toBe(first[0]);
    expect(stabilized[1]).not.toBe(first[1]);
    if (stabilized[1].type === 'assistant') {
      expect(stabilized[1].message.content).toBe('partial plus more');
    }
  });

  it('reuses unchanged tool items inside a changed tools group', () => {
    const toolCall = (id: number, toolCallId: string, status: string) =>
      message({
        id,
        role: 'tool_call',
        content: JSON.stringify({ name: 'Read', input: { file_path: `/repo/${toolCallId}.ts` } }),
        acpEventKind: 'tool_call',
        acpToolCallId: toolCallId,
        acpToolStatus: status,
      });

    const settled = toolCall(1, 'tc-1', 'completed');
    const first = transcript([settled, toolCall(2, 'tc-2', 'in_progress')], []);
    const next = transcript([settled, toolCall(2, 'tc-2', 'completed')], []);

    const stabilized = stabilizeAcpTranscriptGroups(first, next);

    expect(stabilized[0]).not.toBe(first[0]);
    if (stabilized[0].type === 'tools' && first[0].type === 'tools') {
      expect(stabilized[0].items[0]).toBe(first[0].items[0]);
      expect(stabilized[0].items[1]).not.toBe(first[0].items[1]);
      expect(stabilized[0].items[1].status).toBe('completed');
    }
  });

  it('detects in-place metadata changes on an existing tool row', () => {
    const call = message({
      id: 1,
      role: 'tool_call',
      content: JSON.stringify({ name: 'Run', input: { command: 'npm test' } }),
      acpEventKind: 'tool_call',
      acpToolCallId: 'tc-1',
      acpToolStatus: 'in_progress',
    });
    const first = transcript([call], [call]);
    const updated = { ...call, acpToolStatus: 'completed', acpRawOutput: { exitCode: 0 } };
    const next = transcript([call], [updated]);

    const stabilized = stabilizeAcpTranscriptGroups(first, next);

    expect(stabilized[0]).not.toBe(first[0]);
    if (stabilized[0].type === 'tools') {
      expect(stabilized[0].items[0].status).toBe('completed');
    }
  });
});

describe('toolHasDetails', () => {
  it('matches the formatted-value emptiness checks', () => {
    expect(toolHasDetails(richTool({ key: 'tool:1', verb: 'Ran' }))).toBe(false);
    expect(toolHasDetails(richTool({ key: 'tool:1', verb: 'Ran', rawInput: '' }))).toBe(false);
    expect(toolHasDetails(richTool({ key: 'tool:1', verb: 'Ran', rawInput: { a: 1 } }))).toBe(true);
    expect(toolHasDetails(richTool({ key: 'tool:1', verb: 'Ran', rawOutput: 'ok' }))).toBe(true);
    expect(
      toolHasDetails(
        richTool({
          key: 'tool:1',
          verb: 'Ran',
          content: [{ type: 'diff', path: '/repo/a.ts', newText: 'new' }],
        })
      )
    ).toBe(true);
    expect(
      toolHasDetails(
        richTool({ key: 'tool:1', verb: 'Ran', content: [{ type: 'terminal', terminalId: 't-1' }] })
      )
    ).toBe(true);
    expect(
      toolHasDetails(
        richTool({ key: 'tool:1', verb: 'Ran', locations: [{ path: '/repo/a.ts', line: 3 }] })
      )
    ).toBe(true);
    expect(
      toolHasDetails(
        richTool({
          key: 'tool:1',
          verb: 'Ran',
          result: message({ id: 9, role: 'tool_result', content: 'output' }),
        })
      )
    ).toBe(true);
    // Presence-only fields that the expanded card ignores stay hidden.
    expect(
      toolHasDetails(richTool({ key: 'tool:1', verb: 'Ran', content: [{ type: 'diff' }] }))
    ).toBe(false);
    expect(toolHasDetails(richTool({ key: 'tool:1', verb: 'Ran', locations: [{}] }))).toBe(false);
  });
});

describe('isToolMetadataSettled', () => {
  it('treats terminal statuses as settled and everything else as mutable', () => {
    const row = (status?: string) =>
      message({ id: 1, role: 'assistant', acpToolCallId: 'tc-1', acpToolStatus: status });

    expect(isToolMetadataSettled(row('completed'))).toBe(true);
    expect(isToolMetadataSettled(row('failed'))).toBe(true);
    expect(isToolMetadataSettled(row('cancelled'))).toBe(true);
    expect(isToolMetadataSettled(row('in_progress'))).toBe(false);
    expect(isToolMetadataSettled(row('pending'))).toBe(false);
    expect(isToolMetadataSettled(row(undefined))).toBe(false);
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
    pikchrRenderSource: null,
    ...overrides,
  };
}
