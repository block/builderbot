import { describe, expect, it } from 'vitest';
import { formatNoteTimestamp, isNoteRevised, noteLastWrittenAt } from './noteTimestamps';

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;
const now = Date.UTC(2026, 9, 30, 12, 0, 0);

describe('note timestamps', () => {
  it('labels and sorts an unrevised session note by when it completed', () => {
    const completedAt = now - 5 * MINUTE;
    const note = { createdAt: now - 20 * MINUTE, updatedAt: completedAt, completedAt };

    expect(isNoteRevised(note)).toBe(false);
    expect(noteLastWrittenAt(note)).toBe(completedAt);
    expect(formatNoteTimestamp(note, now)).toBe('5m ago');
  });

  it('labels and sorts a revised note by its last update', () => {
    const note = {
      createdAt: now - 3 * DAY,
      updatedAt: now - 2 * 60 * MINUTE,
      completedAt: now - 2 * DAY,
    };

    expect(isNoteRevised(note)).toBe(true);
    expect(noteLastWrittenAt(note)).toBe(note.updatedAt);
    expect(formatNoteTimestamp(note, now)).toBe('Updated 2h ago');
  });

  it('treats an edited written note as revised', () => {
    // Written notes start with all three timestamps equal; edits move only updatedAt.
    const writtenAt = now - DAY;
    const note = { createdAt: writtenAt, updatedAt: now - 10 * MINUTE, completedAt: writtenAt };

    expect(isNoteRevised(note)).toBe(true);
    expect(noteLastWrittenAt(note)).toBe(note.updatedAt);
    expect(formatNoteTimestamp(note, now)).toBe('Updated 10m ago');
  });

  it('falls back to createdAt for a generating stub', () => {
    const note = { createdAt: now - 3 * MINUTE, updatedAt: now - MINUTE, completedAt: null };

    expect(isNoteRevised(note)).toBe(false);
    expect(noteLastWrittenAt(note)).toBe(note.createdAt);
    expect(formatNoteTimestamp(note, now)).toBe('3m ago');
  });

  it('does not treat a failed stub as revised', () => {
    // The session ended without content, so completedAt moved past the stub's updatedAt.
    const note = {
      createdAt: now - 30 * MINUTE,
      updatedAt: now - 30 * MINUTE,
      completedAt: now - 25 * MINUTE,
    };

    expect(isNoteRevised(note)).toBe(false);
    expect(noteLastWrittenAt(note)).toBe(note.completedAt);
    expect(formatNoteTimestamp(note, now)).toBe('25m ago');
  });

  it('keeps the prefix for "just now"', () => {
    const note = { createdAt: now - DAY, updatedAt: now - 10_000, completedAt: now - DAY };

    expect(formatNoteTimestamp(note, now)).toBe('Updated just now');
  });

  it('keeps the prefix past the 7-day locale-date fallback', () => {
    const updatedAt = now - 8 * DAY;
    const note = { createdAt: now - 10 * DAY, updatedAt, completedAt: now - 9 * DAY };

    expect(formatNoteTimestamp(note, now)).toBe(
      `Updated ${new Date(updatedAt).toLocaleDateString()}`
    );
  });
});
