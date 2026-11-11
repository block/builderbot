import { describe, expect, it } from 'vitest';
import { startedByLabel } from './startedBy';

describe('startedByLabel', () => {
  it('is null for sessions the user started', () => {
    expect(startedByLabel(null)).toBeNull();
    expect(startedByLabel(undefined)).toBeNull();
    expect(startedByLabel('')).toBeNull();
  });

  it('names a spawning session by a short id prefix', () => {
    expect(startedByLabel('session:0f1e2d3c-4b5a-6978-8a9b-c0d1e2f3a4b5')).toBe(
      'Started by session 0f1e2d3c…'
    );
    expect(startedByLabel('session:short')).toBe('Started by session short');
  });

  it('names a connected app by its grant', () => {
    expect(startedByLabel('app:9a8b7c6d-5e4f-3a2b-1c0d-e1f2a3b4c5d6')).toBe(
      'Started by a connected app (grant 9a8b7c6d…)'
    );
  });

  it('still renders an actor kind it does not know', () => {
    expect(startedByLabel('robot:xyz')).toBe('Started by robot xyz');
    expect(startedByLabel('opaque')).toBe('Started by opaque');
    expect(startedByLabel('trailing:')).toBe('Started by trailing:');
  });
});
