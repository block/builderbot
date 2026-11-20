import { describe, expect, it } from 'vitest';
import type { Session, SessionMessage } from '../../types';
import {
  buildAcpTranscriptGroups,
  type AcpTranscriptOptions,
  type ToolStatus,
} from './acpTranscript';
import {
  isFirstNoteDeliveryMessage,
  isNoteCardItem,
  listNoteDeliveries,
  noteCallCardLabel,
  noteMessageKey,
  stabilizeNoteDeliveries,
  type NoteDeliveryOptions,
} from './noteDeliveries';
import { shouldAskForNoteUpdate, type LinkedNoteContext } from './noteFreshness';

/** A row created at the time its id names, so ids read as a clock. */
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
function otherCall(id: number, name = 'Bash') {
  return message({
    id,
    role: 'tool_call',
    content: JSON.stringify({ name, input: { command: 'git status' } }),
    acpEventKind: 'tool_call',
    acpToolCallId: `tc-${id}`,
    acpToolStatus: 'completed',
  });
}

/** The key the transcript builder gives the call row with this id. */
const callKey = (id: number) => `tool:tc-${id}`;

/** A reply in the old shape: text, the separator, then the note. */
const SEPARATOR_NOTE = 'Here is the plan.\n\n---\n\n# Plan\n\nFix the test.';

function deliveries(
  rows: SessionMessage[],
  options: NoteDeliveryOptions = {},
  transcript: AcpTranscriptOptions = {}
) {
  return listNoteDeliveries(buildAcpTranscriptGroups(rows, [], undefined, transcript), options);
}

/** The keys of the calls shown as cards, in transcript order. */
function cards(
  rows: SessionMessage[],
  options: NoteDeliveryOptions = {},
  transcript: AcpTranscriptOptions = {}
): string[] {
  return [...deliveries(rows, options, transcript).cardToolKeys];
}

describe('listNoteDeliveries', () => {
  it('finds nothing in a transcript with no note', () => {
    const none = deliveries([user(1), otherCall(2), assistant(3, 'The tree is clean.')]);
    expect(none.firstKey).toBeNull();
    expect(none.cardToolKeys.size).toBe(0);
  });

  it('shows a saved call as a card', () => {
    const found = deliveries([user(1), noteCall(2), assistant(3, 'Done')]);
    expect([...found.cardToolKeys]).toEqual([callKey(2)]);
    expect(found.firstKey).toBe(callKey(2));
  });

  it.each([
    ['Claude Code', 'mcp__staged__write_note'],
    ['Goose', 'staged: write note'],
    ['Codex', 'mcp.staged.write_note'],
  ])('finds the call as %s names it', (_agent, name) => {
    expect(cards([user(1), noteCall(2, 'completed', name)])).toEqual([callKey(2)]);
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
    expect(cards([user(1), call])).toEqual([callKey(2)]);
  });

  it.each([
    ['rejected', 'failed'],
    ['cancelled by the user', 'cancelled'],
    ['waiting to run', 'pending'],
    ['still running', 'in_progress'],
  ] as const)('keeps the row of a call that was %s', (_label, status) => {
    const found = deliveries([user(1), noteCall(2, status), assistant(3, 'Done')]);
    expect(found.cardToolKeys.size).toBe(0);
    expect(found.firstKey).toBeNull();
  });

  it('keeps the row of an interrupted call', () => {
    // The call never settled and the session ended in error or was cancelled:
    // the row reads "Note not saved".
    const rows = [user(1), noteCall(2, 'in_progress')];
    expect(cards(rows, {}, { sessionSettled: true, sessionCompleted: false })).toEqual([]);
  });

  it('shows a remote call completed from the outbox as a card', () => {
    // The call's update was lost with the connection, but the session
    // completed from the workstation outbox: the call resolves to saved.
    const rows = [user(1), noteCall(2, 'in_progress')];
    expect(cards(rows, {}, { sessionSettled: true, sessionCompleted: true })).toEqual([callKey(2)]);
  });

  it("ignores another server's write_note", () => {
    const rows = [user(1), noteCall(2, 'completed', 'mcp__other__write_note')];
    expect(cards(rows)).toEqual([]);
  });

  it('picks the note call out of a tool group with other items', () => {
    // Codex completes a review item after the call; Claude sometimes issues a
    // shell call beside it. They share the tool group and keep their rows.
    const rows = [user(1), otherCall(2), noteCall(3), otherCall(4, 'Guardian Review')];
    expect(cards(rows)).toEqual([callKey(3)]);
  });

  it('shows a rejected call as a row and the retry that saved as a card', () => {
    const rows = [user(1), noteCall(2, 'failed'), assistant(3, 'Fixing the diagram.'), noteCall(4)];
    const found = deliveries(rows);
    expect([...found.cardToolKeys]).toEqual([callKey(4)]);
    expect(found.firstKey).toBe(callKey(4));
  });

  it('names the first delivery the one that created the note', () => {
    const rows = [
      user(1),
      noteCall(2),
      assistant(3, 'Done'),
      user(4, 'Add a section on the fix'),
      noteCall(5),
      assistant(6, 'Done'),
    ];
    const groups = buildAcpTranscriptGroups(rows, []);
    const found = listNoteDeliveries(groups);
    const items = groups.flatMap((group) => (group.type === 'tools' ? group.items : []));

    expect(items.map((item) => isNoteCardItem(found, item))).toEqual([true, true]);
    expect(items.map((item) => noteCallCardLabel(found, item))).toEqual([
      'Note created',
      'Note updated',
    ]);
  });

  it('never labels a saved call as still being written', () => {
    // The pane's live flag does not reach the label: a running call has its
    // own "Saving note" row, and a completed one is saved.
    const groups = buildAcpTranscriptGroups([user(1), noteCall(2)], []);
    const found = listNoteDeliveries(groups, { sessionLive: true });
    const [item] = groups.flatMap((group) => (group.type === 'tools' ? group.items : []));
    expect(noteCallCardLabel(found, item)).toBe('Note created');
  });

  describe('the separator fallback', () => {
    it('finds an assistant message that holds the separator', () => {
      const found = deliveries([user(1), assistant(2, SEPARATOR_NOTE)]);
      expect(found.firstKey).toBe(noteMessageKey(2));
      expect(isFirstNoteDeliveryMessage(found, 2)).toBe(true);
      expect(found.cardToolKeys.size).toBe(0);
    });

    it('does not take a plain reply for a note', () => {
      const found = deliveries([user(1), assistant(2, 'The test races the cookie.')]);
      expect(found.firstKey).toBeNull();
      expect(isFirstNoteDeliveryMessage(found, 2)).toBe(false);
    });

    it('orders both shapes in one list', () => {
      // A session recorded before the tool, continued after it: the note was
      // created by text, so the saved call is an update.
      const rows = [
        user(1),
        assistant(2, SEPARATOR_NOTE),
        user(3, 'Add a section on the fix'),
        noteCall(4),
        user(5, 'And one on the tests'),
        assistant(6, SEPARATOR_NOTE),
      ];
      const groups = buildAcpTranscriptGroups(rows, []);
      const found = listNoteDeliveries(groups);
      const [item] = groups.flatMap((group) => (group.type === 'tools' ? group.items : []));

      expect(found.firstKey).toBe(noteMessageKey(2));
      expect(isFirstNoteDeliveryMessage(found, 2)).toBe(true);
      expect(isFirstNoteDeliveryMessage(found, 6)).toBe(false);
      expect(noteCallCardLabel(found, item)).toBe('Note updated');
    });

    it('is not compared with the stored note', () => {
      // The rule below is about saved calls. The text path is as it was.
      const found = deliveries([user(1), assistant(2, SEPARATOR_NOTE)], { noteUpdatedAt: 0 });
      expect(found.firstKey).toBe(noteMessageKey(2));
    });
  });

  describe('the final turn, against the stored note', () => {
    const turn = [user(1), noteCall(2), assistant(3, 'Done')];

    it.each([
      ['newer than the call: the turn changed the note', 4, [callKey(2)]],
      ['as old as the call', 2, [callKey(2)]],
      ['older than the call: the note was not changed by it', 1, []],
      ['never stored', 0, []],
    ])('reads a note %s', (_label, noteUpdatedAt, expected) => {
      expect(cards(turn, { noteUpdatedAt })).toEqual(expected);
    });

    it('shows the card when the pane has no note to compare with', () => {
      expect(cards(turn)).toEqual([callKey(2)]);
      expect(cards(turn, { noteUpdatedAt: null })).toEqual([callKey(2)]);
    });

    it('shows the card while the session is running', () => {
      // The note is stored after the turn, and the turn may then wait on
      // background work. Until the session settles there is nothing to
      // compare, so the saved call is a card as soon as it completes.
      expect(cards(turn, { noteUpdatedAt: 1, sessionLive: true })).toEqual([callKey(2)]);
    });

    it('keeps the row once that session settles with the note unchanged', () => {
      expect(cards(turn, { noteUpdatedAt: 1, sessionLive: false })).toEqual([]);
    });

    it('does not compare the calls of earlier turns', () => {
      // The first turn stored the note at 4. The second republished it
      // unchanged, which keeps the timestamp: only that call keeps its row.
      const rows = [
        ...turn,
        user(5, 'Is the plan still right?'),
        noteCall(6),
        assistant(7, 'Done'),
      ];
      const found = deliveries(rows, { noteUpdatedAt: 4 });
      expect([...found.cardToolKeys]).toEqual([callKey(2)]);
      expect(found.firstKey).toBe(callKey(2));
    });

    it('leaves an earlier card alone when the final turn has no call', () => {
      // A chat question after the note: the note is older than everything in
      // the final turn, and the earlier call is still the card it was.
      const rows = [...turn, user(5, 'Why that order?'), assistant(6, 'The cookie is set first.')];
      expect(cards(rows, { noteUpdatedAt: 4 })).toEqual([callKey(2)]);
    });

    it('treats a transcript with no user message as one turn', () => {
      // The window can start after the prompt of a long turn.
      const rows = [noteCall(2), assistant(3, 'Done')];
      expect(cards(rows, { noteUpdatedAt: 1 })).toEqual([]);
      expect(cards(rows, { noteUpdatedAt: 4 })).toEqual([callKey(2)]);
    });

    it('reads each saved call of the turn on its own', () => {
      const rows = [user(1), noteCall(2), assistant(3, 'One more section.'), noteCall(4)];
      expect(cards(rows, { noteUpdatedAt: 5 })).toEqual([callKey(2), callKey(4)]);
      expect(cards(rows, { noteUpdatedAt: 1 })).toEqual([]);
    });

    it('has no first delivery when the only call kept its row', () => {
      // A first run that failed before the note was stored.
      expect(deliveries(turn, { noteUpdatedAt: 0 }).firstKey).toBeNull();
    });

    /** Whether the pane offers "Ask for the note to be updated" under the turn. */
    function buttonShown(rows: SessionMessage[], noteUpdatedAt: number): boolean {
      const session = { status: 'completed', completionReason: 'turn_complete' } as Session;
      const note: LinkedNoteContext = {
        id: 'note-1',
        title: 'Plan',
        content: 'Fix the test.',
        updatedAt: noteUpdatedAt,
        hasParsedNote: true,
      };
      return shouldAskForNoteUpdate(session, rows, note);
    }

    it.each([
      ['a turn that changed the note', 4],
      ['a note stored with the closing message', 3],
      ['an identical republish', 1],
      ['a note that was never stored', 0],
    ])('is the complement of the follow-up button for %s', (_label, noteUpdatedAt) => {
      // The button reads the same timestamp: it is offered when the newest
      // assistant message is newer than the note. The note is stored after
      // the turn, so a turn that ends with a saved call shows one of the two.
      const card = cards(turn, { noteUpdatedAt }).length > 0;
      expect(card).toBe(!buttonShown(turn, noteUpdatedAt));
    });

    it('shows both when the agent wrote more after the note was stored', () => {
      // Not a state a turn reaches by itself: the note is stored once the
      // run ends. A message that arrives later, with no prompt before it, is
      // newer than the note. The call did change the note, so it is a card,
      // and the new text is not in the note, so the button is offered.
      const rows = [...turn, assistant(5, 'The background check has finished: all green.')];
      expect(cards(rows, { noteUpdatedAt: 4 })).toEqual([callKey(2)]);
      expect(buttonShown(rows, 4)).toBe(true);
    });
  });
});

describe('stabilizeNoteDeliveries', () => {
  const saved = [user(1), noteCall(2), assistant(3, 'Done')];

  it('takes the first result as it is', () => {
    const next = deliveries(saved);
    expect(stabilizeNoteDeliveries(null, next)).toBe(next);
  });

  it('keeps the earlier object when a rebuild changes no delivery', () => {
    const previous = deliveries(saved);
    // The same deliveries, from a transcript that grew by a reply.
    const next = deliveries([...saved, user(4, 'Thanks'), assistant(5, 'Any time.')]);
    expect(next).not.toBe(previous);
    expect(stabilizeNoteDeliveries(previous, next)).toBe(previous);
  });

  it('hands every empty transcript the same object', () => {
    expect(deliveries([user(1)])).toBe(deliveries([user(1), assistant(2, 'Hello')]));
  });

  it('takes the new result when a call becomes a card', () => {
    const previous = deliveries([user(1), noteCall(2, 'in_progress')]);
    const next = deliveries(saved);
    expect(stabilizeNoteDeliveries(previous, next)).toBe(next);
  });

  it('takes the new result when a card goes back to a row', () => {
    const previous = deliveries(saved, { noteUpdatedAt: 1, sessionLive: true });
    const next = deliveries(saved, { noteUpdatedAt: 1, sessionLive: false });
    expect(stabilizeNoteDeliveries(previous, next)).toBe(next);
  });

  it('takes the new result when another call is saved', () => {
    const previous = deliveries(saved);
    const next = deliveries([...saved, user(4), noteCall(5)]);
    expect(stabilizeNoteDeliveries(previous, next)).toBe(next);
  });

  it('takes the new result when the same number of cards has other keys', () => {
    const previous = deliveries([user(1), noteCall(2)]);
    const next = deliveries([user(1), noteCall(3)]);
    expect(stabilizeNoteDeliveries(previous, next)).toBe(next);
  });

  it('takes the new result when only the first delivery differs', () => {
    // An older page of the transcript loads and brings an earlier delivery.
    const previous = deliveries([user(3), noteCall(4)]);
    const next = deliveries([user(1), assistant(2, SEPARATOR_NOTE), user(3), noteCall(4)]);
    expect([...next.cardToolKeys]).toEqual([...previous.cardToolKeys]);
    expect(stabilizeNoteDeliveries(previous, next)).toBe(next);
  });
});
