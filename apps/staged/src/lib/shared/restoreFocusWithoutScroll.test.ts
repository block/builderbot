import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { restoreFocusWithoutScroll } from './restoreFocusWithoutScroll';

// vitest runs in node: `document` is stubbed down to `body` and
// `activeElement`, and elements to `isConnected` and `focus`.

const body = { tagName: 'BODY' };
let fakeDocument: { body: unknown; activeElement: unknown };

function trigger() {
  return { tagName: 'BUTTON', isConnected: true, focus: vi.fn() };
}

function closeEvent(): Event {
  return new Event('focusScope.onCloseAutoFocus', { cancelable: true });
}

beforeEach(() => {
  fakeDocument = { body, activeElement: body };
  vi.stubGlobal('document', fakeDocument);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('restoreFocusWithoutScroll', () => {
  it('returns focus to the trigger without scrolling, in place of the default', () => {
    const button = trigger();
    fakeDocument.activeElement = button;
    const focusRestore = restoreFocusWithoutScroll(() => true);

    focusRestore.capture();
    fakeDocument.activeElement = body;
    const event = closeEvent();
    focusRestore.restore(event);

    expect(event.defaultPrevented).toBe(true);
    expect(button.focus).toHaveBeenCalledWith({ preventScroll: true });
  });

  it('leaves the default restore alone while disabled', () => {
    const button = trigger();
    fakeDocument.activeElement = button;
    const focusRestore = restoreFocusWithoutScroll(() => false);

    focusRestore.capture();
    const event = closeEvent();
    focusRestore.restore(event);

    expect(event.defaultPrevented).toBe(false);
    expect(button.focus).not.toHaveBeenCalled();
  });

  it('leaves focus alone when the caller already handled it', () => {
    const button = trigger();
    fakeDocument.activeElement = button;
    const focusRestore = restoreFocusWithoutScroll(() => true);

    focusRestore.capture();
    const event = closeEvent();
    event.preventDefault();
    focusRestore.restore(event);

    expect(button.focus).not.toHaveBeenCalled();
  });

  it('skips a trigger that has left the document', () => {
    const button = trigger();
    fakeDocument.activeElement = button;
    const focusRestore = restoreFocusWithoutScroll(() => true);

    focusRestore.capture();
    button.isConnected = false;
    focusRestore.restore(closeEvent());

    expect(button.focus).not.toHaveBeenCalled();
  });

  it('records nothing when only <body> had focus', () => {
    const focusRestore = restoreFocusWithoutScroll(() => true);

    focusRestore.capture();
    const event = closeEvent();

    expect(() => focusRestore.restore(event)).not.toThrow();
    expect(event.defaultPrevented).toBe(true);
  });

  it('restores once per open', () => {
    const button = trigger();
    fakeDocument.activeElement = button;
    const focusRestore = restoreFocusWithoutScroll(() => true);

    focusRestore.capture();
    focusRestore.restore(closeEvent());
    focusRestore.restore(closeEvent());

    expect(button.focus).toHaveBeenCalledTimes(1);
  });
});
