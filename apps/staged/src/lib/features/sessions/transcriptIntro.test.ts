import { describe, expect, it } from 'vitest';
import { createIntroGate } from './transcriptIntro';

describe('createIntroGate', () => {
  it('is off until the opening load enables it', () => {
    const gate = createIntroGate();
    expect(gate.animate).toBe(false);
    gate.enable();
    expect(gate.animate).toBe(true);
  });

  it('suspends intros until the release runs', () => {
    const gate = createIntroGate();
    gate.enable();
    const resume = gate.suspend();
    expect(gate.animate).toBe(false);
    resume();
    expect(gate.animate).toBe(true);
  });

  it('keeps intros on after overlapping suspensions, whichever releases last', () => {
    // Two reopen revalidations within one poll round trip: a captured
    // `animate` flag would be restored as false by the second one.
    const gate = createIntroGate();
    gate.enable();
    const first = gate.suspend();
    const second = gate.suspend();
    first();
    expect(gate.animate).toBe(false);
    second();
    expect(gate.animate).toBe(true);

    const a = gate.suspend();
    const b = gate.suspend();
    b();
    expect(gate.animate).toBe(false);
    a();
    expect(gate.animate).toBe(true);
  });

  it('ignores a second release of the same suspension', () => {
    const gate = createIntroGate();
    gate.enable();
    const resume = gate.suspend();
    const other = gate.suspend();
    resume();
    resume();
    expect(gate.animate).toBe(false);
    other();
    expect(gate.animate).toBe(true);
  });

  it('does not turn on while suspended just because the load enabled it', () => {
    const gate = createIntroGate();
    const resume = gate.suspend();
    gate.enable();
    expect(gate.animate).toBe(false);
    resume();
    expect(gate.animate).toBe(true);
  });
});
