import { describe, it, expect, beforeEach } from 'vitest';
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
  beforeEach(() => {
    localStorage.clear();
  });
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
    act(() => result.current.updateActiveTab('/in-review', 'In Review'));
    expect(result.current.tabs[0].path).toBe('/in-review');
    expect(result.current.tabs[0].title).toBe('In Review');
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

// E-PENPAL-TAB-PERSIST: verifies tab state persistence to localStorage.
describe('useTabs persistence', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('saves tab state to localStorage keyed by window label', async () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    // In browser mode the key is penpal:tabs:browser
    act(() => result.current.openTab('/in-review', 'In Review'));

    // Wait for the async label resolution + save effect
    await act(async () => {
      await new Promise(r => setTimeout(r, 50));
    });

    const key = 'penpal:tabs:browser';
    const raw = localStorage.getItem(key);
    expect(raw).toBeTruthy();
    const parsed = JSON.parse(raw!);
    expect(parsed.version).toBe(1);
    expect(parsed.tabs).toHaveLength(2);
    expect(parsed.tabs[1].path).toBe('/in-review');
  });

  it('generates unique tab IDs using randomUUID', () => {
    const { result } = renderHook(() => useTabs(), { wrapper });
    act(() => result.current.openTab('/in-review'));
    const ids = result.current.tabs.map(t => t.id);
    expect(ids[0]).not.toBe(ids[1]);
    // Both should start with 'tab-'
    expect(ids[0]).toMatch(/^tab-/);
    expect(ids[1]).toMatch(/^tab-/);
  });

  // E-PENPAL-TAB-PERSIST: restores valid persisted tabs from localStorage.
  it('restores persisted tabs from localStorage', () => {
    const persistedTabs = [
      { id: 'tab-aaa', path: '/recent', title: 'Recent', history: ['/recent'], historyIndex: 0 },
      { id: 'tab-bbb', path: '/in-review', title: 'In Review', history: ['/in-review'], historyIndex: 0 },
    ];
    localStorage.setItem('penpal:tabs:browser', JSON.stringify({ version: 1, activeTabId: 'tab-bbb', tabs: persistedTabs }));
    const reviewWrapper = ({ children }: { children: ReactNode }) =>
      createElement(MemoryRouter, { initialEntries: ['/in-review'] }, children);
    const { result } = renderHook(() => useTabs(), { wrapper: reviewWrapper });
    expect(result.current.tabs).toHaveLength(2);
    expect(result.current.tabs[0].path).toBe('/recent');
    expect(result.current.tabs[0].title).toBe('Recent');
    expect(result.current.tabs[1].path).toBe('/in-review');
    expect(result.current.tabs[1].title).toBe('In Review');
    expect(result.current.activeTabId).toBe('tab-bbb');
  });

  // E-PENPAL-SESSION-FALLBACK: corrupt localStorage gracefully falls back.
  it('falls back to default tab when localStorage is corrupt', () => {
    localStorage.setItem('penpal:tabs:browser', 'not-json');
    const { result } = renderHook(() => useTabs(), { wrapper });
    expect(result.current.tabs).toHaveLength(1);
    expect(result.current.tabs[0].path).toBe('/recent');
  });

  it('falls back to default tab when localStorage has empty tabs array', () => {
    localStorage.setItem('penpal:tabs:browser', JSON.stringify({ version: 1, activeTabId: 'x', tabs: [] }));
    const { result } = renderHook(() => useTabs(), { wrapper });
    expect(result.current.tabs).toHaveLength(1);
    expect(result.current.tabs[0].path).toBe('/recent');
  });

  it('falls back to default tab when localStorage has wrong version', () => {
    localStorage.setItem('penpal:tabs:browser', JSON.stringify({ version: 99, activeTabId: 'x', tabs: [{ id: 'x', path: '/', title: 'Home', history: ['/'], historyIndex: 0 }] }));
    const { result } = renderHook(() => useTabs(), { wrapper });
    expect(result.current.tabs).toHaveLength(1);
    expect(result.current.tabs[0].path).toBe('/recent');
  });
});
