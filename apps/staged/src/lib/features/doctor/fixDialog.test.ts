import { describe, expect, it } from 'vitest';
import { closingFixDialogCancelsLogin } from './fixDialog';

describe('closingFixDialogCancelsLogin', () => {
  it('cancels a running login the dialog started', () => {
    // Nothing else is watching it; left alone it would hold the check's login
    // slot until doctor's fix timeout.
    expect(closingFixDialogCancelsLogin({ running: true, startedHere: true })).toBe(true);
  });

  it('detaches from a running login the dialog only attached to', () => {
    // Started from the session pane, another client, or before a reload: its
    // starter is still watching it, and a look through this dialog must not
    // kill it on the way out.
    expect(closingFixDialogCancelsLogin({ running: true, startedHere: false })).toBe(false);
  });

  it('has nothing to cancel when no login is running', () => {
    expect(closingFixDialogCancelsLogin({ running: false, startedHere: true })).toBe(false);
    expect(closingFixDialogCancelsLogin({ running: false, startedHere: false })).toBe(false);
  });
});
