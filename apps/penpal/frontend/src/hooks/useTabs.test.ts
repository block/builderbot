import { describe, it, expect } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { createElement, type ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { useTabs, deriveTitleFromPath } from './useTabs';

function wrapper({ children }: { children: ReactNode }) {
  return createElement(MemoryRouter, { initialEntries: ['/recent'] }, children);
}

describe('deriveTitleFromPath', () => {
  it('derives title from file path', () => {
    expect(deriveTitleFromPath('/file/Development/birdseye/thoughts/doc.md')).toBe('doc.md');
  });

  it('derives title from project path', () => {
    expect(deriveTitleFromPath('/project/Workspace/Project')).toBe('Project');
  });

  it('derives title from workspace path', () => {
    expect(deriveTitleFromPath('/workspace/MyWorkspace')).toBe('MyWorkspace');
  });

  it('derives title from search path', () => {
    expect(deriveTitleFromPath('/search?q=foo')).toBe('Search: foo');
  });

  it('derives title for search without query', () => {
    expect(deriveTitleFromPath('/search')).toBe('Search');
  });

  it('derives title for recent', () => {
    expect(deriveTitleFromPath('/recent')).toBe('Recent');
  });

  it('derives title for in-review', () => {
    expect(deriveTitleFromPath('/in-review')).toBe('In Review');
  });

  it('derives title for home', () => {
    expect(deriveTitleFromPath('/')).toBe('Home');
  });
});

// E-PENPAL-TABS: verifies tab lifecycle, history stacks, back/forward, and tab switching.
describe('useTabs', () => {
  it('initializes with one tab from current URL', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    expect(result.current.tabs).toHaveLength(1);
    expect(result.current.tabs[0].path).toBe('/recent');
    expect(result.current.tabs[0].title).toBe('Recent');
    expect(result.current.activeTabId).toBe(result.current.tabs[0].id);
  });

  it('openTab adds a new tab and sets it active', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    act(() => result.current.openTab('/in-review', 'In Review'));
    expect(result.current.tabs).toHaveLength(2);
    expect(result.current.tabs[1].path).toBe('/in-review');
    expect(result.current.activeTabId).toBe(result.current.tabs[1].id);
  });

  it('closeTab activates neighbor', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    act(() => result.current.openTab('/in-review'));
    const secondTabId = result.current.tabs[1].id;
    const firstTabId = result.current.tabs[0].id;

    // Close the second (active) tab — should activate first
    act(() => result.current.closeTab(secondTabId));
    expect(result.current.tabs).toHaveLength(1);
    expect(result.current.activeTabId).toBe(firstTabId);
  });

  it('cannot close the last tab', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    const tabId = result.current.tabs[0].id;
    act(() => result.current.closeTab(tabId));
    expect(result.current.tabs).toHaveLength(1);
  });

  it('activateTab switches active tab', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    act(() => result.current.openTab('/in-review'));
    const firstTabId = result.current.tabs[0].id;
    act(() => result.current.activateTab(firstTabId));
    expect(result.current.activeTabId).toBe(firstTabId);
  });

  it('updateActiveTab syncs path and title', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    act(() => result.current.updateActiveTab('/search?q=test', 'Search: test'));
    expect(result.current.tabs[0].path).toBe('/search?q=test');
    expect(result.current.tabs[0].title).toBe('Search: test');
  });

  it('initializes tab with history', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    const tab = result.current.tabs[0];
    expect(tab.history).toEqual(['/recent']);
    expect(tab.historyIndex).toBe(0);
  });

  it('openTab initializes new tab with single history entry', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    act(() => result.current.openTab('/in-review'));
    const newTab = result.current.tabs[1];
    expect(newTab.history).toEqual(['/in-review']);
    expect(newTab.historyIndex).toBe(0);
  });

  it('canGoBack and canGoForward are false for new tab', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    expect(result.current.canGoBack).toBe(false);
    expect(result.current.canGoForward).toBe(false);
  });

  it('goBack and goForward are no-ops at boundaries', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    // Should not throw
    act(() => result.current.goBack());
    act(() => result.current.goForward());
    expect(result.current.tabs[0].path).toBe('/recent');
    expect(result.current.tabs[0].historyIndex).toBe(0);
  });

  it('activateTab on already-active tab does not corrupt navigating flag', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    const tabId = result.current.tabs[0].id;
    // Re-activate same tab — should be a no-op for navigation
    act(() => result.current.activateTab(tabId));
    expect(result.current.activeTabId).toBe(tabId);
    expect(result.current.tabs[0].path).toBe('/recent');
    expect(result.current.tabs[0].historyIndex).toBe(0);
  });
});
