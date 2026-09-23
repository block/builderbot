import { describe, expect, it } from 'vitest';
import { closingFixDialogCancelsLogin } from './fixDialog';

describe('closingFixDialogCancelsLogin', () => {
  it('cancels a running login the dialog started', () => {
    // Nothing else is watching it; left alone it would hold the check's login
    // slot until doctor's fix timeout.
    expect(
      closingFixDialogCancelsLogin({ running: true, requestedHere: true, origin: 'started' })
    ).toBe(true);
  });

  it('cancels a login the dialog asked for that the backend has yet to answer', () => {
    // This dialog's request; the store's cancel waits for the run to be named.
    expect(closingFixDialogCancelsLogin({ running: true, requestedHere: true, origin: null })).toBe(
      true
    );
  });

  it('detaches from a login the dialog asked for but the backend answered “already running”', () => {
    // The probe on open found nothing, someone else started a login before Run
    // was clicked, and the click re-attached to theirs. The dialog's own flag
    // says "started here"; the record's origin says otherwise, and it wins.
    expect(
      closingFixDialogCancelsLogin({ running: true, requestedHere: true, origin: 'attached' })
    ).toBe(false);
  });

  it('detaches from a running login the dialog only attached to', () => {
    // Started from the session pane, another client, or before a reload: its
    // starter is still watching it, and a look through this dialog must not
    // kill it on the way out — whatever the record says of how this client
    // came to follow it. The pane's own start reads `started`, and this dialog
    // did not make it.
    for (const origin of ['started', 'attached', null] as const) {
      expect(closingFixDialogCancelsLogin({ running: true, requestedHere: false, origin })).toBe(
        false
      );
    }
  });

  it('has nothing to cancel when no login is running', () => {
    for (const requestedHere of [true, false]) {
      for (const origin of ['started', 'attached', null] as const) {
        expect(closingFixDialogCancelsLogin({ running: false, requestedHere, origin })).toBe(false);
      }
    }
  });
});
