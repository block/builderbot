import { describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useProjectSort } from './useProjectSort';

beforeEach(() => {
  localStorage.clear();
});

// E-PENPAL-HOME-SIDEBAR: verifies default, toggle, persistence, and cross-instance sync.
describe('useProjectSort', () => {
  it('defaults to alpha', () => {
    const { result } = renderHook(() => useProjectSort());
    expect(result.current.sortOrder).toBe('alpha');
  });

  it('reads stored preference', () => {
    localStorage.setItem('penpal-project-sort', 'recent');
    const { result } = renderHook(() => useProjectSort());
    expect(result.current.sortOrder).toBe('recent');
  });

  it('falls back to alpha for invalid stored value', () => {
    localStorage.setItem('penpal-project-sort', 'bogus');
    const { result } = renderHook(() => useProjectSort());
    expect(result.current.sortOrder).toBe('alpha');
  });

  it('toggle switches between alpha and recent', () => {
    const { result } = renderHook(() => useProjectSort());

    act(() => result.current.toggle());
    expect(result.current.sortOrder).toBe('recent');
    expect(localStorage.getItem('penpal-project-sort')).toBe('recent');

    act(() => result.current.toggle());
    expect(result.current.sortOrder).toBe('alpha');
    expect(localStorage.getItem('penpal-project-sort')).toBe('alpha');
  });

  it('setSortOrder persists to localStorage', () => {
    const { result } = renderHook(() => useProjectSort());

    act(() => result.current.setSortOrder('recent'));
    expect(result.current.sortOrder).toBe('recent');
    expect(localStorage.getItem('penpal-project-sort')).toBe('recent');
  });

  it('multiple hook instances stay in sync', () => {
    const { result: a } = renderHook(() => useProjectSort());
    const { result: b } = renderHook(() => useProjectSort());

    act(() => a.current.setSortOrder('recent'));
    expect(b.current.sortOrder).toBe('recent');

    act(() => b.current.toggle());
    expect(a.current.sortOrder).toBe('alpha');
  });
});
