import { describe, expect, it } from 'vitest';
import { assertBitsConfigContext, bitsConfigContextEntries } from './bitsConfigContext';

/** Mirrors bits-ui 2.19.3: `getBitsConfig()` returns the state's `opts` object. */
class FakeBitsConfigState {
  opts = { defaultPortalTo: { current: '#portal-root' }, defaultLocale: { current: undefined } };
}
const configKey = Symbol('BitsConfig');

function contextsUnder(config: FakeBitsConfigState): Map<unknown, unknown> {
  return new Map<unknown, unknown>([
    [Symbol('DialogRoot'), { opts: { open: { current: true } }, contentNode: null }],
    [configKey, config],
    ['string-key', { value: 1 }],
    [Symbol('nullish'), null],
  ]);
}

describe('bitsConfigContextEntries', () => {
  it('copies exactly the entry whose opts is what getBitsConfig() returns', () => {
    const config = new FakeBitsConfigState();
    const copied = bitsConfigContextEntries(contextsUnder(config), config.opts);
    expect(copied.size).toBe(1);
    expect(copied.get(configKey)).toBe(config);
    expect(() => assertBitsConfigContext(copied, true)).not.toThrow();
  });

  it('leaves the adopter dialog state and unrelated contexts behind', () => {
    const config = new FakeBitsConfigState();
    const copied = bitsConfigContextEntries(contextsUnder(config), config.opts);
    expect([...copied.keys()]).toEqual([configKey]);
  });

  it('matches nothing when the config resolved to a fallback or a copy', () => {
    const config = new FakeBitsConfigState();
    const fallback = new FakeBitsConfigState();
    expect(bitsConfigContextEntries(contextsUnder(config), fallback.opts).size).toBe(0);
    expect(bitsConfigContextEntries(contextsUnder(config), { ...config.opts }).size).toBe(0);
  });
});

describe('assertBitsConfigContext', () => {
  it('throws in dev when no entry was copied', () => {
    expect(() => assertBitsConfigContext(new Map(), true)).toThrow(/exactly one bits-ui config/);
  });

  it('throws in dev when the shape match is no longer specific', () => {
    const two = new Map<unknown, unknown>([
      ['a', {}],
      ['b', {}],
    ]);
    expect(() => assertBitsConfigContext(two, true)).toThrow(/found 2/);
  });

  it('is silent outside dev', () => {
    expect(() => assertBitsConfigContext(new Map(), false)).not.toThrow();
  });
});
