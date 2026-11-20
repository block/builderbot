import { describe, expect, it } from 'vitest';
import { writeNoteToolNameCases } from './testStubs/noteDoneCases';
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

  it('reads a Codex call, whose title is `mcp.staged.write_note`', () => {
    expect(
      writeNoteToolCall(content('mcp.staged.write_note', { title: 'From Codex' }), undefined)
    ).toEqual({
      title: 'From Codex',
      suggestedNextCommitStep: null,
      suggestedNextNoteStep: null,
    });
    expect(writeNoteToolCall('mcp.staged.write_note', undefined)?.title).toBe('');
  });

  it('is null for any other tool', () => {
    expect(writeNoteToolCall(content('mcp__other__write_note', { title: 'Plan' }), undefined)).toBe(
      null
    );
    expect(
      writeNoteToolCall(content('mcp.other.write_note', { title: 'Plan' }), undefined)
    ).toBeNull();
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
