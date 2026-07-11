import { describe, expect, it } from 'vitest';
import { mergeAcpConfigPref, preferredAcpEffort } from './acpConfigPrefs';

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

  it('records the effort under its model alongside the provider-level effort', () => {
    expect(
      mergeAcpConfigPref({ modelEfforts: { haiku: 'low' } }, { effort: 'max', effortModel: 'opus' })
    ).toEqual({
      effort: 'max',
      modelEfforts: { haiku: 'low', opus: 'max' },
    });
  });

  it('clearing an effort with a model also clears that model entry', () => {
    expect(
      mergeAcpConfigPref(
        { effort: 'max', modelEfforts: { opus: 'max' } },
        { effort: null, effortModel: 'opus' }
      )
    ).toEqual({});
  });

  it('does not mutate the current modelEfforts', () => {
    const current = { modelEfforts: { opus: 'max' } };
    mergeAcpConfigPref(current, { effort: 'low', effortModel: 'opus' });
    expect(current).toEqual({ modelEfforts: { opus: 'max' } });
  });
});

describe('preferredAcpEffort', () => {
  it('prefers the effort recorded for the model', () => {
    expect(preferredAcpEffort({ effort: 'max', modelEfforts: { haiku: 'low' } }, 'haiku')).toBe(
      'low'
    );
  });

  it('falls back to the provider-level effort for other models', () => {
    expect(preferredAcpEffort({ effort: 'max', modelEfforts: { haiku: 'low' } }, 'opus')).toBe(
      'max'
    );
  });

  it('uses the provider-level effort when no model is known', () => {
    expect(preferredAcpEffort({ effort: 'max', modelEfforts: { haiku: 'low' } }, null)).toBe('max');
  });

  it('returns null without a pref or any matching effort', () => {
    expect(preferredAcpEffort(null, 'opus')).toBeNull();
    expect(preferredAcpEffort({ model: 'opus' }, 'opus')).toBeNull();
  });
});
