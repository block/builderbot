import { describe, expect, it } from 'vitest';
import type { Session, SessionStatus } from '../../types';
import { StatusHold } from './statusHold';

function row(status: SessionStatus, rest: Partial<Session> = {}): Session {
  return {
    id: 's1',
    status,
    errorMessage: null,
    completionReason: null,
    updatedAt: 1,
    ...rest,
  } as Session;
}

/**
 * A pane as far as its status goes: the event handler's patch, and the row
 * a frame's session row turns into.
 */
function pane(initial: Session) {
  const hold = new StatusHold();
  let shown = initial;
  return {
    hold,
    get shown() {
      return shown;
    },
    event(status: SessionStatus, rest: Partial<Session> = {}) {
      shown = { ...shown, status, ...rest };
      return hold.event(status);
    },
    frame(session: Session) {
      shown = hold.frame(session, shown);
    },
  };
}

describe('StatusHold', () => {
  it('takes the status from frames while no event has been applied', () => {
    const p = pane(row('running'));
    p.frame(row('completed', { updatedAt: 2 }));
    expect(p.shown).toEqual(row('completed', { updatedAt: 2 }));
    expect(p.hold.holding).toBe(false);
  });

  it('does not let an older frame revert the status an event brought', () => {
    const p = pane(row('running'));
    expect(p.event('completed', { completionReason: 'end_turn' as never })).toBe(true);

    // Read before the status was written, delivered after its event.
    p.frame(row('running', { updatedAt: 5 }));
    expect(p.shown.status).toBe('completed');
    expect(p.shown.completionReason).toBe('end_turn');
    expect(p.hold.holding).toBe(true);
  });

  it('still adopts the rest of a frame’s row while it holds the status', () => {
    const p = pane(row('running'));
    p.event('completed');
    p.frame(row('running', { acpTitle: 'Renamed', updatedAt: 5 } as Partial<Session>));

    expect(p.shown.status).toBe('completed');
    expect(p.shown.acpTitle).toBe('Renamed');
    expect(p.shown.updatedAt).toBe(5);
  });

  it('takes the status from frames again from the first that agrees', () => {
    const p = pane(row('running'));
    p.event('completed');
    p.frame(row('running'));
    const agreeing = row('completed', { updatedAt: 9 });
    p.frame(agreeing);
    expect(p.shown).toBe(agreeing);
    expect(p.hold.holding).toBe(false);

    // Nothing is held any more, so a status no event announced (another
    // Staged process wrote it) is taken as it comes.
    p.frame(row('cancelled'));
    expect(p.shown.status).toBe('cancelled');
  });

  it('holds nothing for an event the frames had already caught up with', () => {
    const p = pane(row('running'));
    p.frame(row('completed'));
    expect(p.event('completed')).toBe(false);

    // The next turn's frame overtakes its event too, and is not mistaken
    // for an older one.
    p.frame(row('running'));
    expect(p.shown.status).toBe('running');
    expect(p.event('running')).toBe(false);
  });

  it('ends on the last event when the frames overtook two of them', () => {
    const p = pane(row('running'));
    p.frame(row('completed'));
    p.frame(row('running'));
    // The events for both writes, late and in order.
    p.event('completed');
    p.event('running');
    expect(p.shown.status).toBe('running');
    expect(p.hold.holding).toBe(false);
  });

  it('holds the newest of two events until its own frame', () => {
    const p = pane(row('running'));
    p.event('completed');
    p.event('running');
    p.frame(row('completed'));
    expect(p.shown.status).toBe('running');
    p.frame(row('running', { updatedAt: 4 }));
    expect(p.shown).toEqual(row('running', { updatedAt: 4 }));
    expect(p.hold.holding).toBe(false);
  });

  it('adopts whatever comes once the hold is released', () => {
    const p = pane(row('running'));
    p.event('completed');
    expect(p.hold.release()).toBe(true);
    expect(p.hold.release()).toBe(false);
    p.frame(row('cancelled'));
    expect(p.shown.status).toBe('cancelled');
  });

  it('forgets what the frames last said on a reset', () => {
    const p = pane(row('running'));
    p.frame(row('completed'));
    p.hold.reset();
    // A new stream has told the pane nothing yet, so the event is held.
    expect(p.event('completed')).toBe(true);
  });
});
