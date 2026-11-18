import { describe, expect, it } from 'vitest';
import type { SessionMessage } from '../../types';
import {
  buildAcpTranscriptGroups,
  type AcpTranscriptGroup,
  type AcpTranscriptOptions,
  type ToolStatus,
} from './acpTranscript';
import { NOTE_DONE_WORD, hiddenNoteDoneMessageIds, isNoteDoneText } from './noteDoneMessage';

describe('isNoteDoneText', () => {
  it.each([
    ['the word as asked for', 'Done'],
    ['a trailing full stop', 'Done.'],
    ['a trailing exclamation mark', 'Done!'],
    ['lower case', 'done'],
    ['upper case', 'DONE'],
    ['surrounding whitespace', '  Done \n'],
    ['a trailing newline, as a streamed chunk ends', 'Done\n\n'],
    ['bold', '**Done**'],
    ['italic, asterisk', '*Done*'],
    ['italic, underscore', '_Done_'],
    ['inline code', '`Done`'],
    ['double quotes', '"Done"'],
    ['single quotes', "'Done'"],
    ['curly double quotes', '“Done”'],
    ['curly single quotes', '‘Done’'],
    ['a wrapper around the punctuation', '**Done.**'],
    ['quotes around the punctuation', '"Done!"'],
    ['a wrapper, lower case, padded', '\n `done` '],
  ])('accepts %s', (_label, text) => {
    expect(isNoteDoneText(text)).toBe(true);
  });

  it.each([
    ['an empty message', ''],
    ['whitespace only', ' \n '],
    ['only punctuation', '.'],
    ['only a wrapper', '****'],
    ['an emoji after the word', 'Done ✅'],
    ['a sentence that starts with the word', 'Done, the note is saved'],
    ['a sentence after the word', 'Done. The note is saved.'],
    ['a sentence that ends with the word', 'The note is done'],
    ['an answer that ends with a Done line', 'The bug is in `src/main.rs`.\n\nDone'],
    ['a Done line before more text', 'Done\n\nOne more thing: the test is flaky.'],
    ['the word twice', 'Done Done'],
    ['two trailing marks', 'Done..'],
    ['an ellipsis', 'Done…'],
    ['another trailing mark', 'Done?'],
    ['two pairs of wrappers', '**`Done`**'],
    ['punctuation outside the wrapper', '**Done**.'],
    ['a doubled underscore', '__Done__'],
    ['an unclosed wrapper', '**Done'],
    ['mismatched quotes', '"Done\''],
    ['whitespace inside the wrapper', '** Done **'],
    ['a longer word', 'Donee'],
    ['another word', 'Finished'],
    ['a heading', '# Done'],
    ['a list item', '- Done'],
  ])('rejects %s', (_label, text) => {
    expect(isNoteDoneText(text)).toBe(false);
  });

  it('rejects a missing text', () => {
    expect(isNoteDoneText(null)).toBe(false);
    expect(isNoteDoneText(undefined)).toBe(false);
  });

  it('accepts the word exactly as it is exported', () => {
    // The tool result quotes this word; the wording tests read it from here.
    expect(NOTE_DONE_WORD).toBe('Done');
    expect(isNoteDoneText(NOTE_DONE_WORD)).toBe(true);
  });
});

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

const user = (id: number, content = 'Write the note') => message({ id, role: 'user', content });
const assistant = (id: number, content: string) => message({ id, role: 'assistant', content });

const NOTE_INPUT = { title: 'Plan' };

/** A `write_note` call row, in Claude Code's spelling unless `name` says otherwise. */
function noteCall(id: number, status: ToolStatus = 'completed', name = 'mcp__staged__write_note') {
  return message({
    id,
    role: 'tool_call',
    content: JSON.stringify({ name, input: NOTE_INPUT }),
    acpEventKind: 'tool_call',
    acpToolCallId: `tc-${id}`,
    acpToolStatus: status,
    acpRawInput: NOTE_INPUT,
  });
}

/** Any other tool call row. */
function otherCall(id: number, status: ToolStatus = 'completed', name = 'Bash') {
  return message({
    id,
    role: 'tool_call',
    content: JSON.stringify({ name, input: { command: 'git status' } }),
    acpEventKind: 'tool_call',
    acpToolCallId: `tc-${id}`,
    acpToolStatus: status,
  });
}

function hiddenIds(rows: SessionMessage[], options: AcpTranscriptOptions = {}): number[] {
  return [...hiddenNoteDoneMessageIds(buildAcpTranscriptGroups(rows, [], undefined, options))];
}

describe('hiddenNoteDoneMessageIds', () => {
  it('hides a bare Done after a saved call', () => {
    expect(hiddenIds([user(1), noteCall(2), assistant(3, 'Done')])).toEqual([3]);
  });

  it.each([
    ['Claude Code', 'mcp__staged__write_note'],
    ['Goose', 'staged: write note'],
    ['Codex', 'mcp.staged.write_note'],
  ])('finds the call as %s names it', (_agent, name) => {
    expect(hiddenIds([user(1), noteCall(2, 'completed', name), assistant(3, 'Done')])).toEqual([3]);
  });

  it('finds a call whose row holds only the tool name', () => {
    // No arguments on the row: recognised by name, not by its label.
    const call = message({
      id: 2,
      role: 'tool_call',
      content: 'mcp.staged.write_note',
      acpEventKind: 'tool_call',
      acpToolCallId: 'tc-2',
      acpToolStatus: 'completed',
    });
    expect(hiddenIds([user(1), call, assistant(3, 'Done')])).toEqual([3]);
  });

  it('hides the tolerated spellings of the word', () => {
    expect(hiddenIds([user(1), noteCall(2), assistant(3, '**Done.**\n')])).toEqual([3]);
  });

  it('keeps the reply written before the call', () => {
    const rows = [
      user(1, 'Which file holds the bug?'),
      assistant(2, 'The bug is in `src/main.rs`.'),
      noteCall(3),
      assistant(4, 'Done'),
    ];
    expect(hiddenIds(rows)).toEqual([4]);
  });

  it('shows the message after a rejected call', () => {
    expect(hiddenIds([user(1), noteCall(2, 'failed'), assistant(3, 'Done')])).toEqual([]);
  });

  it('shows the message while the call is still running', () => {
    expect(hiddenIds([user(1), noteCall(2, 'in_progress'), assistant(3, 'Done')])).toEqual([]);
    expect(hiddenIds([user(1), noteCall(2, 'pending'), assistant(3, 'Done')])).toEqual([]);
  });

  it('shows the message after an interrupted call', () => {
    // The call never settled and the session ended in error or was cancelled:
    // the row reads "Note not saved".
    const rows = [user(1), noteCall(2, 'in_progress'), assistant(3, 'Done')];
    expect(hiddenIds(rows, { sessionSettled: true, sessionCompleted: false })).toEqual([]);
  });

  it('shows the message after a call the user cancelled', () => {
    expect(hiddenIds([user(1), noteCall(2, 'cancelled'), assistant(3, 'Done')])).toEqual([]);
  });

  it('hides it after a remote call completed from the outbox', () => {
    // The call's update was lost with the connection, but the session
    // completed from the workstation outbox: the row resolves to "Saved note".
    const rows = [user(1), noteCall(2, 'in_progress'), assistant(3, 'Done')];
    expect(hiddenIds(rows, { sessionSettled: true, sessionCompleted: true })).toEqual([3]);
  });

  it('shows the message when a tool call follows it', () => {
    const rows = [user(1), noteCall(2), assistant(3, 'Done'), otherCall(4)];
    expect(hiddenIds(rows)).toEqual([]);
  });

  it('shows the message when a tool call and more text follow it', () => {
    const rows = [user(1), noteCall(2), assistant(3, 'Done'), otherCall(4), assistant(5, 'Done')];
    // The last one follows a tool group with no `write_note` item.
    expect(hiddenIds(rows)).toEqual([]);
  });

  it('shows both messages when more text follows the Done', () => {
    const rows = [
      user(1),
      noteCall(2),
      assistant(3, 'Done'),
      assistant(4, 'The note covers the three failing tests.'),
    ];
    expect(hiddenIds(rows)).toEqual([]);
  });

  it('shows both messages when a reply comes between the call and the Done', () => {
    const rows = [
      user(1),
      noteCall(2),
      assistant(3, 'The bug is on line 12 of `src/main.rs`.'),
      assistant(4, 'Done'),
    ];
    expect(hiddenIds(rows)).toEqual([]);
  });

  it('shows text that continues after the word in the same message', () => {
    expect(hiddenIds([user(1), noteCall(2), assistant(3, 'Done, the note is saved.')])).toEqual([]);
  });

  it('shows an answer that ends with a Done line, whole', () => {
    const rows = [user(1), noteCall(2), assistant(3, 'The bug is in `src/main.rs`.\n\nDone')];
    expect(hiddenIds(rows)).toEqual([]);
  });

  it('hides two Done messages in a row', () => {
    // A background task woke the session after the turn and the agent
    // answered with a second one.
    const rows = [user(1), noteCall(2), assistant(3, 'Done'), assistant(4, 'Done.')];
    expect(hiddenIds(rows)).toEqual([3, 4]);
  });

  it("ignores Codex's extra item after the call in the same tool group", () => {
    // Codex announces a review item after `write_note` and completes it
    // before the call's own result; both land in one tool group.
    const review = message({
      id: 3,
      role: 'tool_call',
      content: 'Guardian Review',
      acpEventKind: 'tool_call',
      acpToolCallId: 'tc-review',
      acpToolStatus: 'completed',
    });
    const rows = [
      user(1),
      noteCall(2, 'completed', 'mcp.staged.write_note'),
      review,
      assistant(4, 'Done'),
    ];
    const groups = buildAcpTranscriptGroups(rows, []);
    expect(groups.map((group) => group.type)).toEqual(['user', 'tools', 'assistant']);
    expect([...hiddenNoteDoneMessageIds(groups)]).toEqual([4]);
  });

  it('ignores a shell call issued beside the note call', () => {
    const rows = [user(1), otherCall(2), noteCall(3), otherCall(4, 'failed'), assistant(5, 'Done')];
    expect(hiddenIds(rows)).toEqual([5]);
  });

  it('goes by the last write_note item of the tool group', () => {
    // Rejected, fixed, saved: the last call stands.
    expect(hiddenIds([user(1), noteCall(2, 'failed'), noteCall(3), assistant(4, 'Done')])).toEqual([
      4,
    ]);
    // Saved, then a second call rejected: the note row above reads "Note rejected".
    expect(hiddenIds([user(1), noteCall(2), noteCall(3, 'failed'), assistant(4, 'Done')])).toEqual(
      []
    );
  });

  it('shows a Done that no note call precedes', () => {
    expect(hiddenIds([user(1), assistant(2, 'Done')])).toEqual([]);
    expect(hiddenIds([user(1), otherCall(2), assistant(3, 'Done')])).toEqual([]);
    expect(hiddenIds([assistant(1, 'Done')])).toEqual([]);
  });

  it("shows a Done after another server's write_note", () => {
    const rows = [
      user(1),
      noteCall(2, 'completed', 'mcp__other__write_note'),
      assistant(3, 'Done'),
    ];
    expect(hiddenIds(rows)).toEqual([]);
  });

  it('shows a Done when a saved call is separated from it by a user message', () => {
    const rows = [user(1), noteCall(2), user(3, 'Are you finished?'), assistant(4, 'Done')];
    expect(hiddenIds(rows)).toEqual([]);
  });

  it('hides the Done of every turn that ends with one', () => {
    const rows = [
      user(1),
      noteCall(2),
      assistant(3, 'Done'),
      user(4, 'Which file holds the bug?'),
      assistant(5, 'It is `src/main.rs`.'),
      user(6, 'Add that to the note'),
      noteCall(7),
      assistant(8, 'done'),
    ];
    expect(hiddenIds(rows)).toEqual([3, 8]);
  });

  it('judges each turn on its own', () => {
    const rows = [
      user(1),
      noteCall(2),
      assistant(3, 'Done'),
      user(4, 'Add the file name'),
      noteCall(5),
      assistant(6, 'Done. I added the file name under "Cause".'),
    ];
    expect(hiddenIds(rows)).toEqual([3]);
  });

  it('hides nothing in a turn that ends at the call', () => {
    expect(hiddenIds([user(1), noteCall(2)])).toEqual([]);
    expect(hiddenIds([])).toEqual([]);
  });

  it('steps over acp groups, which render nothing', () => {
    const [userGroup, toolsGroup, doneGroup] = buildAcpTranscriptGroups(
      [user(1), noteCall(2), assistant(4, 'Done')],
      []
    );
    const acp = (id: number): AcpTranscriptGroup => ({
      type: 'acp',
      event: {
        id,
        kind: 'usage_update',
        title: 'Usage',
        content: null,
        message: message({ id, role: 'assistant' }),
      },
    });
    expect([
      ...hiddenNoteDoneMessageIds([userGroup, toolsGroup, acp(3), doneGroup, acp(5)]),
    ]).toEqual([4]);
  });
});
