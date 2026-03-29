import { describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useProjectSort } from './useProjectSort';

beforeEach(() => {
  localStorage.clear();
});

// E-PENPAL-SORT, E-PENPAL-VIEW-OPTIONS: verifies default, toggle, persistence, showEmpty, and cross-instance sync.
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

  // E-PENPAL-VIEW-OPTIONS: showEmpty tests
  it('defaults showEmpty to true', () => {
    const { result } = renderHook(() => useProjectSort());
    expect(result.current.showEmpty).toBe(true);
  });

  it('reads stored showEmpty preference', () => {
    localStorage.setItem('penpal-show-empty', 'false');
    const { result } = renderHook(() => useProjectSort());
    expect(result.current.showEmpty).toBe(false);
  });

  it('setShowEmpty persists to localStorage', () => {
    const { result } = renderHook(() => useProjectSort());

    act(() => result.current.setShowEmpty(false));
    expect(result.current.showEmpty).toBe(false);
    expect(localStorage.getItem('penpal-show-empty')).toBe('false');

    act(() => result.current.setShowEmpty(true));
    expect(result.current.showEmpty).toBe(true);
    expect(localStorage.getItem('penpal-show-empty')).toBe('true');
  });

  it('showEmpty syncs across instances', () => {
    const { result: a } = renderHook(() => useProjectSort());
    const { result: b } = renderHook(() => useProjectSort());

    act(() => a.current.setShowEmpty(false));
    expect(b.current.showEmpty).toBe(false);
  });
});
