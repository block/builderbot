import { describe, expect, it } from 'vitest';
import {
  isWriteNoteToolName,
  writeNoteCallFromInput,
  writeNoteToolCall,
  writeNoteVerb,
} from './noteToolCall';

const steps = {
  suggestedNextCommitStep: 'Implement this plan',
  suggestedNextNoteStep: null,
};

describe('isWriteNoteToolName', () => {
  it.each([
    'mcp__staged__write_note',
    'staged: write note',
    'staged.write_note',
    'staged_write_note',
  ])('recognises %s', (name) => {
    expect(isWriteNoteToolName(name)).toBe(true);
  });

  it.each(['mcp__other__write_note', 'write_note', 'Write', '', null, undefined])(
    'rejects %s',
    (name) => {
      expect(isWriteNoteToolName(name)).toBe(false);
    }
  );
});

describe('writeNoteCallFromInput', () => {
  it('reads the title and the suggested next steps', () => {
    expect(writeNoteCallFromInput({ title: '  Plan  ', suggested_next_steps: steps })).toEqual({
      title: 'Plan',
      suggestedNextCommitStep: 'Implement this plan',
      suggestedNextNoteStep: null,
    });
  });

  it('accepts the camel-case and snake-case spellings of the steps', () => {
    expect(
      writeNoteCallFromInput({
        title: 'Plan',
        suggestedNextSteps: {
          suggested_next_commit_step: ' Fix this bug ',
          suggested_next_note_step: 'Plan a fix',
        },
      })
    ).toEqual({
      title: 'Plan',
      suggestedNextCommitStep: 'Fix this bug',
      suggestedNextNoteStep: 'Plan a fix',
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

  it('reads a call with steps but no title yet', () => {
    expect(writeNoteCallFromInput({ suggested_next_steps: steps })).toEqual({
      title: '',
      suggestedNextCommitStep: 'Implement this plan',
      suggestedNextNoteStep: null,
    });
  });

  it.each([
    ['nothing', undefined],
    ['null', null],
    ['an array', ['Plan']],
    ['a string', 'Plan'],
    ['an empty object', {}],
    ['only a body', { body: 'An inline body from before.' }],
    ['a deferred placeholder', { $deferred: { messageId: 1, field: 'input' } }],
  ])('returns null for %s', (_, input) => {
    expect(writeNoteCallFromInput(input)).toBeNull();
  });
});

describe('writeNoteToolCall', () => {
  const content = (name: string, input: unknown) => JSON.stringify({ name, input });

  it('prefers the raw input, then the tool_call content', () => {
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
  });

  it('still marks an argument-less call as a note row', () => {
    expect(writeNoteToolCall('mcp__staged__write_note', undefined)).toEqual({
      title: '',
      suggestedNextCommitStep: null,
      suggestedNextNoteStep: null,
    });
  });

  it('is null for any other tool', () => {
    expect(writeNoteToolCall(content('mcp__other__write_note', { title: 'Plan' }), undefined)).toBe(
      null
    );
    expect(writeNoteToolCall('Bash', { title: 'Plan' })).toBeNull();
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
