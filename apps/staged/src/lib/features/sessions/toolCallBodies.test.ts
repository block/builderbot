import { describe, expect, it } from 'vitest';
import type { SessionMessage, ToolCallBody } from '../../types';
import {
  deferredToolBody,
  deferredToolBodyIds,
  hydrateToolItem,
  retainInlineToolBodies,
} from './toolCallBodies';

const marker = (messageId: number, field: 'input' | 'output' | 'content', extra = {}) => ({
  $deferred: { messageId, field, bytes: 4096, ...extra },
});

describe('deferredToolBody', () => {
  it('parses the store marker and its pikchr hints', () => {
    expect(
      deferredToolBody(marker(42, 'output', { innerSessionId: 'child-1', pikchr: 'box "A"' }))
    ).toEqual({
      messageId: 42,
      field: 'output',
      bytes: 4096,
      innerSessionId: 'child-1',
      pikchr: 'box "A"',
    });
    expect(deferredToolBody(marker(7, 'content'))).toEqual({
      messageId: 7,
      field: 'content',
      bytes: 4096,
      innerSessionId: null,
      pikchr: null,
    });
  });

  it('treats anything that is not a well-formed marker as a body', () => {
    expect(deferredToolBody({ stdout: 'hi' })).toBeNull();
    expect(deferredToolBody([{ $deferred: { messageId: 1, field: 'input' } }])).toBeNull();
    expect(deferredToolBody({ $deferred: { messageId: '1', field: 'input' } })).toBeNull();
    expect(deferredToolBody({ $deferred: { messageId: 1, field: 'source' } })).toBeNull();
    expect(deferredToolBody({ $deferred: 'yes' })).toBeNull();
    expect(deferredToolBody('text')).toBeNull();
    expect(deferredToolBody(null)).toBeNull();
  });
});

describe('deferredToolBodyIds', () => {
  it('lists the rows behind the markers once each, ascending', () => {
    expect(
      deferredToolBodyIds({
        rawInput: marker(9, 'input'),
        rawOutput: marker(3, 'output'),
        content: marker(9, 'content'),
      })
    ).toEqual([3, 9]);
    expect(
      deferredToolBodyIds({ rawInput: { command: 'ls' }, rawOutput: 'ok', content: undefined })
    ).toEqual([]);
  });
});

describe('hydrateToolItem', () => {
  const body: ToolCallBody = {
    messageId: 9,
    acpRawInput: { command: 'cargo test' },
    acpRawOutput: { stdout: 'ok' },
    acpContent: [{ type: 'content', content: { type: 'text', text: 'ok' } }],
  };

  it('swaps each marker for the matching field of its fetched body', () => {
    const item = {
      key: 'tool:1',
      rawInput: marker(9, 'input'),
      rawOutput: marker(9, 'output'),
      content: marker(9, 'content'),
    };
    expect(hydrateToolItem(item, new Map([[9, body]]))).toEqual({
      key: 'tool:1',
      rawInput: body.acpRawInput,
      rawOutput: body.acpRawOutput,
      content: body.acpContent,
    });
  });

  it('leaves inline bodies alone and keeps identity when nothing is deferred', () => {
    const item = { rawInput: { command: 'ls' }, rawOutput: 'listing', content: undefined };
    expect(hydrateToolItem(item, new Map([[9, body]]))).toBe(item);
  });

  it('keeps a marker whose body has not been fetched, and nulls a missing field', () => {
    const item = {
      rawInput: marker(9, 'input'),
      rawOutput: marker(10, 'output'),
      content: marker(9, 'content'),
    };
    const hydrated = hydrateToolItem(
      item,
      new Map([[9, { messageId: 9, acpRawInput: { command: 'ls' } }]])
    );
    expect(hydrated.rawInput).toEqual({ command: 'ls' });
    expect(hydrated.rawOutput).toBe(item.rawOutput);
    expect(hydrated.content).toBeNull();
  });
});

describe('retainInlineToolBodies', () => {
  /** The store measures `bytes` as the UTF-8 length of the serialized body. */
  const serializedBytes = (value: unknown) =>
    new TextEncoder().encode(JSON.stringify(value)).length;

  const toolRow = (
    id: number,
    status: string,
    bodies: Partial<SessionMessage>
  ): SessionMessage => ({
    id,
    sessionId: 's1',
    role: 'tool_call',
    content: '',
    createdAt: id,
    acpEventKind: 'tool_call',
    acpToolCallId: `tc-${id}`,
    acpToolStatus: status,
    ...bodies,
  });

  const input = { file_path: '/tmp/a.txt', content: 'x'.repeat(3000) };
  const output = { stdout: 'é'.repeat(2000) };

  it('keeps the held inline bodies when the settled markers report their size', () => {
    const current = toolRow(5, 'in_progress', { acpRawInput: input, acpRawOutput: output });
    const update = toolRow(5, 'completed', {
      acpRawInput: { $deferred: { messageId: 5, field: 'input', bytes: serializedBytes(input) } },
      acpRawOutput: {
        $deferred: { messageId: 5, field: 'output', bytes: serializedBytes(output) },
      },
      acpContent: [{ type: 'content', content: { type: 'text', text: 'done' } }],
    });

    const retained = retainInlineToolBodies(current, update);

    expect(retained).not.toBe(update);
    expect(retained.acpToolStatus).toBe('completed');
    expect(retained.acpRawInput).toBe(input);
    expect(retained.acpRawOutput).toBe(output);
    expect(retained.acpContent).toBe(update.acpContent);
    expect(update.acpRawInput).toEqual({ $deferred: expect.anything() });
  });

  it('leaves a marker in place when the held body is not the size the store measured', () => {
    const partial = { stdout: output.stdout.slice(0, 1000) };
    const current = toolRow(5, 'in_progress', { acpRawInput: input, acpRawOutput: partial });
    const update = toolRow(5, 'completed', {
      acpRawInput: { $deferred: { messageId: 5, field: 'input', bytes: serializedBytes(input) } },
      acpRawOutput: {
        $deferred: { messageId: 5, field: 'output', bytes: serializedBytes(output) },
      },
    });

    const retained = retainInlineToolBodies(current, update);

    expect(retained.acpRawInput).toBe(input);
    expect(retained.acpRawOutput).toBe(update.acpRawOutput);
  });

  it('returns the update itself when nothing can be kept', () => {
    const marker = { $deferred: { messageId: 5, field: 'input', bytes: serializedBytes(input) } };
    const settled = toolRow(5, 'completed', { acpRawInput: marker });

    // Nothing held for the field, a held marker, a marker for another row or
    // field, a different row, and an update with no markers at all.
    expect(retainInlineToolBodies(toolRow(5, 'in_progress', {}), settled)).toBe(settled);
    expect(retainInlineToolBodies(toolRow(5, 'completed', { acpRawInput: marker }), settled)).toBe(
      settled
    );
    const foreign = toolRow(5, 'completed', {
      acpRawInput: { $deferred: { ...marker.$deferred, messageId: 6 } },
      acpRawOutput: { $deferred: { ...marker.$deferred, field: 'content' } },
    });
    expect(
      retainInlineToolBodies(
        toolRow(5, 'in_progress', { acpRawInput: input, acpRawOutput: input }),
        foreign
      )
    ).toBe(foreign);
    expect(retainInlineToolBodies(toolRow(4, 'in_progress', { acpRawInput: input }), settled)).toBe(
      settled
    );
    const inline = toolRow(5, 'in_progress', { acpRawInput: input });
    expect(retainInlineToolBodies(toolRow(5, 'pending', { acpRawInput: input }), inline)).toBe(
      inline
    );
  });
});
