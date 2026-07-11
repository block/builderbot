import { describe, expect, it } from 'vitest';
import { mergeAcpConfigPref } from './acpConfigPrefs';

describe('mergeAcpConfigPref', () => {
  it('creates a pref from scratch', () => {
    expect(mergeAcpConfigPref(undefined, { model: 'opus' })).toEqual({ model: 'opus' });
  });

  it('updates one field without touching the other', () => {
    expect(mergeAcpConfigPref({ model: 'opus', effort: 'high' }, { effort: 'medium' })).toEqual({
      model: 'opus',
      effort: 'medium',
    });
  });

  it('leaves absent fields untouched', () => {
    expect(mergeAcpConfigPref({ effort: 'high' }, { model: 'opus' })).toEqual({
      model: 'opus',
      effort: 'high',
    });
  });

  it('clears a field with null', () => {
    expect(mergeAcpConfigPref({ model: 'opus', effort: 'high' }, { effort: null })).toEqual({
      model: 'opus',
    });
  });

  it('does not mutate the current pref', () => {
    const current = { model: 'opus' };
    mergeAcpConfigPref(current, { model: 'sonnet' });
    expect(current).toEqual({ model: 'opus' });
  });
});
