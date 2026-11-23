import { describe, expect, it } from 'vitest';
import { writeNoteToolNameCases } from './testStubs/noteDoneCases';
import {
  isWriteNoteToolName,
  writeNoteCallFromInput,
  writeNoteCallHasDetails,
  writeNoteRowDetail,
  writeNoteToolCall,
  writeNoteVerb,
  type WriteNoteCall,
} from './noteToolCall';

const steps = {
  suggestedNextCommitStep: 'Implement this plan',
  suggestedNextNoteStep: null,
};

/** A parsed call: no title and no suggested step unless `overrides` gives one. */
const call = (overrides: Partial<WriteNoteCall> = {}): WriteNoteCall => ({
  title: null,
  suggestedNextCommitStep: null,
  suggestedNextNoteStep: null,
  ...overrides,
});

describe('isWriteNoteToolName', () => {
  // The cases live in testStubs/noteDone.cases.json, which the backend's copy of this
  // rule runs too (src-tauri/src/note_done/tests.rs): the drift log finds the
  // call by the same names as the chat pane.
  it.each(writeNoteToolNameCases.accept)('recognises the %s spelling: %j', (_, name) => {
    expect(isWriteNoteToolName(name)).toBe(true);
  });

  it.each(writeNoteToolNameCases.reject)('rejects %s: %j', (_, name) => {
    expect(isWriteNoteToolName(name)).toBe(false);
  });

  it('rejects a missing name', () => {
    expect(isWriteNoteToolName(null)).toBe(false);
    expect(isWriteNoteToolName(undefined)).toBe(false);
  });
});

describe('writeNoteCallFromInput', () => {
  it('reads the suggested next steps of a call, which has no title', () => {
    expect(writeNoteCallFromInput({ suggested_next_steps: steps })).toEqual({
      title: null,
      suggestedNextCommitStep: 'Implement this plan',
      suggestedNextNoteStep: null,
    });
  });

  it('accepts the camel-case and snake-case spellings of the steps', () => {
    expect(
      writeNoteCallFromInput({
        suggestedNextSteps: {
          suggested_next_commit_step: ' Fix this bug ',
          suggested_next_note_step: 'Plan a fix',
        },
      })
    ).toEqual({
      title: null,
      suggestedNextCommitStep: 'Fix this bug',
      suggestedNextNoteStep: 'Plan a fix',
    });
  });

  it('reads a call whose steps are both null', () => {
    expect(
      writeNoteCallFromInput({
        suggested_next_steps: { suggestedNextCommitStep: null, suggestedNextNoteStep: null },
      })
    ).toEqual({ title: null, suggestedNextCommitStep: null, suggestedNextNoteStep: null });
  });

  // A call recorded before the title moved into the draft file passed it as
  // an argument, and the row is still named after it.
  describe('a call recorded with a title', () => {
    it('reads the title, trimmed, beside the suggested next steps', () => {
      expect(writeNoteCallFromInput({ title: '  Plan  ', suggested_next_steps: steps })).toEqual({
        title: 'Plan',
        suggestedNextCommitStep: 'Implement this plan',
        suggestedNextNoteStep: null,
      });
    });

    it('reads a title that came with no steps', () => {
      expect(writeNoteCallFromInput({ title: 'Plan' })).toEqual({
        title: 'Plan',
        suggestedNextCommitStep: null,
        suggestedNextNoteStep: null,
      });
    });

    it("carries no body: the draft file is the note, and an older row's body is ignored", () => {
      const call = writeNoteCallFromInput({ title: 'Plan', body: 'An inline body from before.' });
      expect(call).toEqual({
        title: 'Plan',
        suggestedNextCommitStep: null,
        suggestedNextNoteStep: null,
      });
      expect(call).not.toHaveProperty('body');
    });

    it.each([
      ['blank', '   '],
      ['null', null],
      ['a number', 12],
      ['an array', ['Plan']],
    ])('reads a title that is %s as no title', (_, title) => {
      expect(writeNoteCallFromInput({ title, suggested_next_steps: steps })).toEqual({
        title: null,
        suggestedNextCommitStep: 'Implement this plan',
        suggestedNextNoteStep: null,
      });
      expect(writeNoteCallFromInput({ title })).toBeNull();
    });
  });

  it.each([
    ['nothing', undefined],
    ['null', null],
    ['an array', ['Plan']],
    ['a string', 'Plan'],
    ['an empty object, the input of a call that suggests no step', {}],
    ['steps that are not an object', { suggested_next_steps: 'Implement this plan' }],
    ['only a body', { body: 'An inline body from before.' }],
    ['a deferred placeholder', { $deferred: { messageId: 1, field: 'input' } }],
  ])('returns null for %s', (_, input) => {
    expect(writeNoteCallFromInput(input)).toBeNull();
  });
});

describe('writeNoteToolCall', () => {
  const content = (name: string, input: unknown) => JSON.stringify({ name, input });
  const otherSteps = { suggestedNextCommitStep: null, suggestedNextNoteStep: 'Plan a fix' };

  it('prefers the raw input, then the tool_call content', () => {
    const fromContent = content('mcp__staged__write_note', { suggested_next_steps: otherSteps });
    expect(writeNoteToolCall(fromContent, { suggested_next_steps: steps })).toEqual({
      title: null,
      suggestedNextCommitStep: 'Implement this plan',
      suggestedNextNoteStep: null,
    });
    expect(
      writeNoteToolCall(content('staged: write note', { suggested_next_steps: otherSteps }), {
        $deferred: { messageId: 1, field: 'input' },
      })
    ).toEqual({ title: null, suggestedNextCommitStep: null, suggestedNextNoteStep: 'Plan a fix' });
  });

  it.each([
    ['a bare tool name', 'mcp__staged__write_note', undefined],
    ['an empty input', content('mcp__staged__write_note', {}), {}],
    ['a Goose call', content('staged: write note', {}), undefined],
    ['a Codex call', 'mcp.staged.write_note', undefined],
  ])('marks a call with no arguments as a note row: %s', (_, toolCall, rawInput) => {
    expect(writeNoteToolCall(toolCall, rawInput)).toEqual(call());
  });

  it('reads a Codex call, whose title is `mcp.staged.write_note`', () => {
    expect(
      writeNoteToolCall(
        content('mcp.staged.write_note', { suggested_next_steps: steps }),
        undefined
      )
    ).toEqual({
      title: null,
      suggestedNextCommitStep: 'Implement this plan',
      suggestedNextNoteStep: null,
    });
  });

  it('reads the title of a call recorded with one, from the raw input first', () => {
    expect(
      writeNoteToolCall(content('mcp__staged__write_note', { title: 'From content' }), {
        title: 'From raw input',
      })?.title
    ).toBe('From raw input');
    expect(
      writeNoteToolCall(content('staged: write note', { title: 'From content' }), {
        $deferred: { messageId: 1, field: 'input' },
      })?.title
    ).toBe('From content');
    expect(
      writeNoteToolCall(content('mcp.staged.write_note', { title: 'From Codex' }), undefined)
    ).toEqual({
      title: 'From Codex',
      suggestedNextCommitStep: null,
      suggestedNextNoteStep: null,
    });
  });

  it('is null for any other tool', () => {
    expect(
      writeNoteToolCall(
        content('mcp__other__write_note', { suggested_next_steps: steps }),
        undefined
      )
    ).toBeNull();
    expect(
      writeNoteToolCall(content('mcp.other.write_note', { title: 'Plan' }), undefined)
    ).toBeNull();
    expect(writeNoteToolCall('Bash', { suggested_next_steps: steps })).toBeNull();
  });
});

describe('writeNoteRowDetail', () => {
  it('is empty for a call with no title, so the row is its verb alone', () => {
    expect(writeNoteRowDetail(call())).toBe('');
    expect(writeNoteRowDetail(call({ suggestedNextCommitStep: 'Implement this plan' }))).toBe('');
  });

  it('is the title of a call recorded with one', () => {
    expect(writeNoteRowDetail(call({ title: 'Plan' }))).toBe('Plan');
  });
});

describe('writeNoteCallHasDetails', () => {
  it('is false for a call with no title and no suggested step', () => {
    expect(writeNoteCallHasDetails(call())).toBe(false);
  });

  it.each([
    ['a next commit step', call({ suggestedNextCommitStep: 'Implement this plan' })],
    ['a next note step', call({ suggestedNextNoteStep: 'Plan a fix' })],
    ['the title of a call recorded with one', call({ title: 'Plan' })],
  ])('is true for %s', (_, note) => {
    expect(writeNoteCallHasDetails(note)).toBe(true);
  });
});

describe('writeNoteVerb', () => {
  it('names the row by status', () => {
    expect(writeNoteVerb('completed')).toBe('Saved note');
    expect(writeNoteVerb('failed')).toBe('Note rejected');
    expect(writeNoteVerb('cancelled')).toBe('Note not saved');
    expect(writeNoteVerb('pending')).toBe('Saving note');
    expect(writeNoteVerb('in_progress')).toBe('Saving note');
  });
});
