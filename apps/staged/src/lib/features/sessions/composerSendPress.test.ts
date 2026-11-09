import { describe, expect, it } from 'vitest';
import { createCompositionSendGate, createTouchPressFocusGuard } from './composerSendPress';

describe('createTouchPressFocusGuard', () => {
  it("cancels a touch press's compatibility mousedown", () => {
    const guard = createTouchPressFocusGuard();
    guard.pointerDown('touch');
    expect(guard.mouseDown(false)).toBe(true);
    guard.reset();
  });

  it('treats a pen like a touch', () => {
    const guard = createTouchPressFocusGuard();
    guard.pointerDown('pen');
    expect(guard.mouseDown(false)).toBe(true);
  });

  it('leaves a mouse press alone', () => {
    const guard = createTouchPressFocusGuard();
    guard.pointerDown('mouse');
    expect(guard.mouseDown(false)).toBe(false);
  });

  it('a mouse press after a touch whose mousedown was suppressed keeps the default', () => {
    // The browser fired no compatibility mousedown for the touch, only the
    // click; a mouse press afterwards must move focus as usual.
    const guard = createTouchPressFocusGuard();
    guard.pointerDown('touch');
    guard.reset(); // its click
    guard.pointerDown('mouse');
    expect(guard.mouseDown(false)).toBe(false);
  });

  it('a stale touch press, ended by neither click nor pointercancel, cancels once', () => {
    // The next mousedown comes with no pointerdown of its own (a VoiceOver
    // activation, say): the stale touch cancels it, and nothing after.
    const guard = createTouchPressFocusGuard();
    guard.pointerDown('touch');

    expect(guard.mouseDown(false)).toBe(true);
    expect(guard.mouseDown(false)).toBe(false);
  });

  it('a touch that turned into a scroll is forgotten', () => {
    const guard = createTouchPressFocusGuard();
    guard.pointerDown('touch');
    guard.reset(); // pointercancel
    expect(guard.mouseDown(false)).toBe(false);
  });

  it('a keyboard or VoiceOver activation, with no pointerdown, is not cancelled', () => {
    // VoiceOver can synthesise a mousedown before its click; with no touch
    // pointerdown recorded, nothing is cancelled. A plain keyboard click
    // fires no mousedown at all and never consults the guard.
    const guard = createTouchPressFocusGuard();
    expect(guard.mouseDown(false)).toBe(false);

    guard.pointerDown('touch');
    expect(guard.mouseDown(false)).toBe(true);
    // The same press's mousedown is consumed; a second one is not cancelled.
    expect(guard.mouseDown(false)).toBe(false);
  });

  it('lets focus move while a composition is open, so it commits', () => {
    const guard = createTouchPressFocusGuard();
    guard.pointerDown('touch');
    expect(guard.mouseDown(true)).toBe(false);
  });
});

describe('createCompositionSendGate', () => {
  it('sends straight away with no composition', () => {
    const gate = createCompositionSendGate();
    expect(gate.press(false)).toBe(true);
    expect(gate.compositionEnded()).toBe(false);
  });

  it('holds a press during a composition until it ends, then sends once', () => {
    const gate = createCompositionSendGate();
    expect(gate.press(true)).toBe(false);
    expect(gate.press(true)).toBe(false);

    expect(gate.compositionEnded()).toBe(true);
    expect(gate.compositionEnded()).toBe(false);
  });

  it('a composition that ends with no press waiting sends nothing', () => {
    const gate = createCompositionSendGate();
    expect(gate.compositionEnded()).toBe(false);
  });

  it('a composition dropped without ending takes the waiting press with it', () => {
    // The box's content replaced mid-composition (a failed send putting the
    // draft back): no compositionend comes, and a later composition's end
    // must not send.
    const gate = createCompositionSendGate();
    gate.press(true);
    gate.cancel();
    expect(gate.compositionEnded()).toBe(false);
  });

  it('a press after the composition ended sends and clears any wait', () => {
    const gate = createCompositionSendGate();
    gate.press(true);
    expect(gate.press(false)).toBe(true);
    expect(gate.compositionEnded()).toBe(false);
  });
});
