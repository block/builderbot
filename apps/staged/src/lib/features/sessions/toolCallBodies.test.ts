import { describe, expect, it } from 'vitest';
import type { ToolCallBody } from '../../types';
import { deferredToolBody, deferredToolBodyIds, hydrateToolItem } from './toolCallBodies';

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
