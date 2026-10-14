import { describe, expect, it } from 'vitest';
import type { SessionMessage } from '../../types';
import type { RichToolItem, ToolStatus } from './acpTranscript';
import {
  buildToolCallViewModel,
  summarizeToolCallLocations,
  type ToolCallLocation,
} from './toolCallViewModel';

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

  it('does not treat generic status output as a network payload', () => {
    const model = buildToolCallViewModel(
      richTool({
        verb: 'Processed',
        rawOutput: { status: 'ok', id: 42, title: 'Done' },
      })
    );

    // A stray `status`/`title` key is not network evidence, so the payload must
    // stay generic and keep its raw fallback instead of vanishing behind the
    // network renderer's structured-response view.
    expect(model.category).toBe('generic');
    expect(model.sections).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: 'raw_output', text: expect.stringContaining('"id": 42') }),
      ])
    );
  });

  it('still classifies unknown tools as network on request/response evidence', () => {
    const model = buildToolCallViewModel(
      richTool({
        verb: 'Processed',
        rawOutput: { response: { status: 200, body: 'ok' } },
      })
    );

    expect(model.category).toBe('network');
  });
});

describe('buildToolCallViewModel output handling', () => {
  const png =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=';
  const acpImage = {
    type: 'content',
    content: { type: 'image', data: png, mimeType: 'image/png' },
  };
  const rawImage = {
    type: 'image',
    source: { type: 'base64', data: png, media_type: 'image/png' },
  };

  it('renders a Claude Read image once without its raw or legacy JSON', () => {
    const model = buildToolCallViewModel(
      richTool({
        toolKind: 'read',
        content: [acpImage],
        rawOutput: [rawImage],
        result: message({ id: 2, role: 'tool_result', content: JSON.stringify([rawImage]) }),
      })
    );

    expect(model.output.images).toEqual([
      { kind: 'inline', dataUrl: `data:image/png;base64,${png}`, label: 'Tool result image' },
    ]);
    expect(model.sections.map((section) => section.kind)).toEqual(['image', 'status']);
    expect(model.output.primaryText).toBe('');
    expect(model.output.emptyLabel).toBeNull();
    expect(model.hasDetails).toBe(true);
  });

  it('renders store-projected image refs without any inline payload', () => {
    // What the server now ships in place of a base64 block, in both the ACP
    // content envelope and Claude's raw output (store/tool_images.rs).
    const model = buildToolCallViewModel(
      richTool({
        toolKind: 'read',
        content: [
          {
            type: 'content',
            content: { type: 'image', ref: '42/content/0', mimeType: 'image/png' },
          },
        ],
        rawOutput: [
          { type: 'text', text: 'Read 1 image' },
          { type: 'image', ref: '42/output/1', mimeType: 'image/png' },
        ],
      })
    );

    expect(model.output.images).toEqual([
      {
        kind: 'ref',
        ref: { ref: '42/content/0', messageId: 42, field: 'content', index: 0 },
        mimeType: 'image/png',
        label: 'Tool result image',
      },
    ]);
    // The sibling text block still renders; only the image payload moved.
    expect(model.sections.map((section) => section.kind)).toEqual(['image', 'output', 'status']);
    expect(model.output.primaryText).toBe('Read 1 image');
    expect(model.sections.some((section) => section.kind === 'raw_output')).toBe(false);
    expect(model.hasDetails).toBe(true);
  });

  it('ignores malformed refs and falls back to inline data when present', () => {
    const model = buildToolCallViewModel(
      richTool({
        rawOutput: [
          { type: 'image', ref: 'not/a/ref', mimeType: 'image/png' },
          { type: 'image', ref: '../etc/passwd', mimeType: 'image/png', data: png },
        ],
      })
    );

    expect(model.output.images).toEqual([
      { kind: 'inline', dataUrl: `data:image/png;base64,${png}`, label: 'Tool result image' },
    ]);
  });

  it('keeps accompanying text and multiple images in ACP output', () => {
    const model = buildToolCallViewModel(
      richTool({
        content: [
          { type: 'content', content: { type: 'text', text: 'Screenshot captured' } },
          acpImage,
          { type: 'content', content: { type: 'image', mimeType: 'image/jpeg', data: 'jpeg' } },
        ],
      })
    );

    expect(model.output.images).toHaveLength(2);
    expect(model.output.primaryText).toBe('Screenshot captured');
    expect(model.output.state).toBe('output');
  });

  it.each([
    ['Claude', [rawImage, { type: 'text', text: 'Image result' }]],
    [
      'MCP',
      {
        content: [
          { type: 'image', data: png, mimeType: 'image/png' },
          { type: 'text', text: 'Image result' },
        ],
      },
    ],
  ])('renders %s raw images when ACP content is absent', (_, rawOutput) => {
    const model = buildToolCallViewModel(richTool({ rawOutput }));

    expect(model.output.images).toHaveLength(1);
    expect(model.output.primaryText).toBe('Image result');
    expect(model.sections.some((section) => section.kind === 'raw_output')).toBe(false);
    expect(model.output.primaryText).not.toContain(png);
  });

  it('renders a Codex View Image resource link without requiring a MIME type', () => {
    const path = '/tmp/screenshots/after.png';
    const model = buildToolCallViewModel(
      richTool({
        toolKind: 'read',
        content: [{ type: 'content', content: { type: 'resource_link', uri: path, name: path } }],
      })
    );

    expect(model.output.images).toEqual([{ kind: 'file', path, label: path }]);
    expect(model.output.state).toBe('output');
    expect(model.output.emptyLabel).toBeNull();
    expect(model.hasDetails).toBe(true);
  });

  it.each(['output', 'text', 'body', 'response', 'result'])(
    'preserves a sibling %s field alongside raw image content',
    (field) => {
      const model = buildToolCallViewModel(
        richTool({
          rawOutput: { [field]: 'Screenshot captured', content: [rawImage] },
        })
      );

      expect(model.output.primaryText).toBe('Screenshot captured');
      expect(model.sections).toContainEqual({
        kind: 'output',
        source: 'primary',
        label: 'Output',
        text: 'Screenshot captured',
        tone: 'normal',
      });
      expect(model.sections.some((section) => section.kind === 'raw_output')).toBe(false);
    }
  );

  it('keeps distinct sibling fields and image captions without repeating identical text', () => {
    const model = buildToolCallViewModel(
      richTool({
        rawOutput: {
          output: 'Screenshot captured',
          text: 'Screenshot captured',
          body: { width: 1280, height: 720 },
          content: [rawImage, { type: 'text', text: '```text\nImage caption\n```' }],
        },
      })
    );

    expect(model.output.primaryText).toBe(
      'Screenshot captured\n{\n  "width": 1280,\n  "height": 720\n}\nImage caption'
    );
  });

  it('filters image payloads in the selected output field while preserving its text', () => {
    const model = buildToolCallViewModel(
      richTool({
        content: [acpImage],
        rawOutput: { body: [rawImage, { type: 'text', text: 'Screenshot captured' }] },
      })
    );

    expect(model.output.primaryText).toBe('Screenshot captured');
    expect(model.output.images).toHaveLength(1);
  });

  it('preserves a raw text response accompanying an ACP image', () => {
    const model = buildToolCallViewModel(
      richTool({ content: [acpImage], rawOutput: { body: 'Screenshot captured' } })
    );

    expect(model.output.images).toHaveLength(1);
    expect(model.output.primaryText).toBe('Screenshot captured');
  });

  it('decodes file URLs for image resource links', () => {
    const model = buildToolCallViewModel(
      richTool({
        content: [
          {
            type: 'content',
            content: { type: 'resource_link', uri: 'file:///tmp/screen%20shot.PNG' },
          },
        ],
      })
    );

    expect(model.output.images).toEqual([
      { kind: 'file', path: '/tmp/screen shot.PNG', label: '/tmp/screen shot.PNG' },
    ]);
  });

  it('leaves unsupported and malformed raw content available as JSON', () => {
    const rawOutput = [
      null,
      { type: 'image', data: png },
      { type: 'image', data: '', mimeType: 'image/png' },
      { type: 'image', data: png, mimeType: 'text/html' },
      { type: 'resource_link', uri: '/tmp/document.txt' },
      { type: 'resource_link', uri: 'https://example.test/screenshot.png' },
      { type: 'resource_link', uri: 'file://remote-host/screenshot.png' },
      { type: 'resource_link', uri: 'file:///tmp/%invalid.png' },
    ];
    const model = buildToolCallViewModel(richTool({ rawOutput }));

    expect(model.output.images).toEqual([]);
    expect(model.sections).toContainEqual({
      kind: 'raw_output',
      label: 'Raw output',
      text: JSON.stringify(rawOutput, null, 2),
    });
  });

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

  it('strips wrapping markdown fences from ACP content text', () => {
    const model = buildToolCallViewModel(
      richTool({
        toolKind: 'read',
        content: [
          {
            type: 'content',
            content: { type: 'text', text: '```\n1952\tasync fn list()\n1953\t{}\n```' },
          },
        ],
      })
    );

    expect(model.output.primaryText).toBe('1952\tasync fn list()\n1953\t{}');
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
      source: 'primary',
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

  it('is not expandable when only empty and status rows would show', () => {
    const model = buildToolCallViewModel(
      richTool({
        status: 'completed',
        result: null,
      })
    );

    expect(model.output.state).toBe('empty');
    expect(model.output.emptyLabel).toBe('No output');
    expect(model.hasDetails).toBe(false);
    expect(model.sections).toContainEqual({ kind: 'empty', label: 'No output' });
  });

  it('is expandable once any structured detail exists', () => {
    const model = buildToolCallViewModel(richTool({ rawInput: { option: 'value' } }));

    expect(model.hasDetails).toBe(true);
  });

  it('is expandable when a body was deferred by the store, before it is fetched', () => {
    // The store ships settled bodies past its size threshold as markers; the
    // collapsed card must look exactly as it would with the body inline —
    // expandable, with output — and nothing may pretty-print the marker.
    const marker = (field: 'input' | 'output' | 'content') => ({
      $deferred: { messageId: 42, field, bytes: 9000 },
    });

    const outputOnly = buildToolCallViewModel(richTool({ rawOutput: marker('output') }));
    expect(outputOnly.hasDetails).toBe(true);
    expect(outputOnly.output.state).toBe('output');
    expect(outputOnly.output.images).toEqual([]);
    expect(outputOnly.output.errorText).toBe('');
    // Classification comes from the verb here, as it would with the body
    // inline; the marker's keys never read as network evidence.
    expect(outputOnly.category).toBe('command');

    // ACP content alone contributes nothing to the sections until it is a
    // real block list, so the marker is what keeps the card expandable.
    const contentOnly = buildToolCallViewModel(richTool({ content: marker('content') }));
    expect(contentOnly.hasDetails).toBe(true);
    expect(contentOnly.metadata.diffs).toEqual([]);

    const inputOnly = buildToolCallViewModel(richTool({ rawInput: marker('input') }));
    expect(inputOnly.hasDetails).toBe(true);
    expect(inputOnly.metadata.input).toBeNull();
    expect(inputOnly.metadata.hasInputText).toBe(false);
    expect(inputOnly.metadata.command).toBeNull();
    expect(inputOnly.metadata.targetPath).toBeNull();
  });

  it('reads a deferred input from the tool_call content copy so the card does not reclassify on expand', () => {
    // The visible `tool_call.content` is `{"name","input":<raw_input>}`, the
    // same input the store deferred, so the collapsed card must classify and
    // extract metadata from it exactly as it will once the body is fetched.
    const input = { command: 'curl https://example.com/api', cwd: '/repo/apps' };
    const call = message({
      id: 1,
      role: 'tool_call',
      content: JSON.stringify({ name: 'shell', input }),
    });
    const deferred = buildToolCallViewModel(
      richTool({
        call,
        verb: 'Processed',
        rawInput: { $deferred: { messageId: 1, field: 'input', bytes: 9000 } },
      }),
      '/repo'
    );
    const hydrated = buildToolCallViewModel(
      richTool({ call, verb: 'Processed', rawInput: input }),
      '/repo'
    );

    expect(deferred.metadata.input).toEqual(input);
    expect(deferred.metadata.command).toBe('curl https://example.com/api');
    expect(deferred.metadata.workingDirectory).toBe('apps');
    expect(deferred.metadata.parsedCommands).toEqual(hydrated.metadata.parsedCommands);
    expect(deferred.category).toBe(hydrated.category);
    expect(deferred.metadata.hasInputText).toBe(true);
    expect(deferred.metadata.inputText).toBe(hydrated.metadata.inputText);
    expect(deferred.metadata.inputText).not.toContain('$deferred');
  });

  it('defers raw output JSON formatting until the value is read', () => {
    let reads = 0;
    const rawOutput = {
      get payload() {
        reads += 1;
        return 'value';
      },
    };

    const model = buildToolCallViewModel(richTool({ verb: 'Processed', rawOutput }));

    // Building the view model for a collapsed card must not pretty-print the
    // raw output; that only happens once an expanded card reads `rawText`.
    expect(reads).toBe(0);
    expect(model.output.hasRawText).toBe(true);
    expect(model.hasDetails).toBe(true);
    expect(reads).toBe(0);

    expect(model.output.rawText).toContain('value');
    expect(reads).toBe(1);
  });
});

describe('summarizeToolCallLocations', () => {
  function location(overrides: Partial<ToolCallLocation> = {}): ToolCallLocation {
    const path = overrides.path ?? 'src/a.ts';
    const line = overrides.line ?? null;
    const column = overrides.column ?? null;
    return {
      path,
      display: `${path}${line !== null ? `:${line}` : ''}${column !== null ? `:${column}` : ''}`,
      line,
      column,
      ...overrides,
    };
  }

  it('folds the target path location into a line suffix instead of a chip', () => {
    const summary = summarizeToolCallLocations([location({ line: 1952 })], 'src/a.ts');

    expect(summary.pathSuffix).toBe(':1952');
    expect(summary.chips).toEqual([]);
  });

  it('includes columns in the folded suffix', () => {
    const summary = summarizeToolCallLocations([location({ line: 12, column: 4 })], 'src/a.ts');

    expect(summary.pathSuffix).toBe(':12:4');
  });

  it('collapses extra covered-path locations to line labels and keeps other paths in full', () => {
    const summary = summarizeToolCallLocations(
      [location({ line: 12 }), location({ line: 40 }), location({ path: 'src/b.ts', line: 3 })],
      'src/a.ts'
    );

    expect(summary.pathSuffix).toBe(':12');
    expect(summary.chips).toEqual(['Line 40', 'src/b.ts:3']);
  });

  it('drops covered locations that add no line info', () => {
    const summary = summarizeToolCallLocations([location()], 'src/a.ts');

    expect(summary.pathSuffix).toBe('');
    expect(summary.chips).toEqual([]);
  });

  it('keeps line labels for paths covered elsewhere when no path field is shown', () => {
    const summary = summarizeToolCallLocations([location({ line: 12 })], null, ['src/a.ts']);

    expect(summary.pathSuffix).toBe('');
    expect(summary.chips).toEqual(['Line 12']);
  });
});
