import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useTheme } from './useTheme';

beforeEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
});

// E-PENPAL-THEME: verifies localStorage, system preference, and toggle behavior.
describe('useTheme', () => {
  it('defaults to light when no preference', () => {
    vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: false } as MediaQueryList);
    const { result } = renderHook(() => useTheme());
    expect(result.current.theme).toBe('light');
    expect(document.documentElement.getAttribute('data-theme')).toBeNull();
  });

  it('reads from localStorage', () => {
    localStorage.setItem('penpal-theme', 'dark');
    const { result } = renderHook(() => useTheme());
    expect(result.current.theme).toBe('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
  });

  it('toggles between light and dark', () => {
    vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: false } as MediaQueryList);
    const { result } = renderHook(() => useTheme());

    act(() => result.current.toggle());
    expect(result.current.theme).toBe('dark');
    expect(localStorage.getItem('penpal-theme')).toBe('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');

    act(() => result.current.toggle());
    expect(result.current.theme).toBe('light');
    expect(localStorage.getItem('penpal-theme')).toBe('light');
    expect(document.documentElement.getAttribute('data-theme')).toBeNull();
  });

  it('respects system dark preference', () => {
    vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: true } as MediaQueryList);
    const { result } = renderHook(() => useTheme());
    expect(result.current.theme).toBe('dark');
  });
});
