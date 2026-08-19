// @vitest-environment jsdom
import 'fake-indexeddb/auto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Mock transport — web mode
vi.mock('../transport', () => ({
  isTauri: false,
  invokeCommand: vi.fn(),
}));

// Spy on markAllStale
const mockMarkAllStale = vi.fn().mockResolvedValue(undefined);
vi.mock('../cache', () => ({
  markAllStale: (...args: unknown[]) => mockMarkAllStale(...args),
}));

import {
  listenForPageLifecycle,
  _setLastActivityTimestamp,
  _getLastActivityTimestamp,
  _setHiddenAt,
  _STALE_THRESHOLD_MS,
} from './pageLifecycleListener';

describe('pageLifecycleListener', () => {
  let unlisten: () => void;
  let cacheStaleEvents: Event[];
  let projectNotesEvents: Event[];

  function onCacheStale(e: Event) {
    cacheStaleEvents.push(e);
  }

  function onProjectNotesInvalidated(e: Event) {
    projectNotesEvents.push(e);
  }

  beforeEach(() => {
    mockMarkAllStale.mockClear();
    cacheStaleEvents = [];
    projectNotesEvents = [];
    window.addEventListener('cache-stale', onCacheStale);
    window.addEventListener('project-notes-invalidated', onProjectNotesInvalidated);
    _setLastActivityTimestamp(Date.now());
    // Reset the hide timestamp so each test starts from "no recorded hide",
    // which (safely) revalidates on resume.
    _setHiddenAt(0);
    unlisten = listenForPageLifecycle();
  });

  afterEach(() => {
    unlisten();
    window.removeEventListener('cache-stale', onCacheStale);
    window.removeEventListener('project-notes-invalidated', onProjectNotesInvalidated);
  });

  describe('resume event', () => {
    it('marks all cache entries stale and dispatches cache-stale', async () => {
      document.dispatchEvent(new Event('resume'));

      // markAllStale is async, give it a tick
      await vi.waitFor(() => {
        expect(mockMarkAllStale).toHaveBeenCalledTimes(1);
      });
      expect(cacheStaleEvents).toHaveLength(1);
      // Project notes have no cache-stale consumer of their own.
      expect(projectNotesEvents).toHaveLength(1);
    });

    it('updates lastActivityTimestamp after resume', async () => {
      _setLastActivityTimestamp(0);
      const before = Date.now();
      document.dispatchEvent(new Event('resume'));

      await vi.waitFor(() => {
        expect(mockMarkAllStale).toHaveBeenCalled();
      });
      expect(_getLastActivityTimestamp()).toBeGreaterThanOrEqual(before);
    });
  });

  describe('visibilitychange event', () => {
    it('marks stale when returning after >30s gap', async () => {
      // Simulate being hidden for longer than the threshold
      _setLastActivityTimestamp(Date.now() - _STALE_THRESHOLD_MS - 1000);

      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true,
      });
      document.dispatchEvent(new Event('visibilitychange'));

      await vi.waitFor(() => {
        expect(mockMarkAllStale).toHaveBeenCalledTimes(1);
      });
      expect(cacheStaleEvents).toHaveLength(1);
    });

    it('does NOT mark stale when returning within 30s', async () => {
      // Activity was recent
      _setLastActivityTimestamp(Date.now() - 1000);

      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true,
      });
      document.dispatchEvent(new Event('visibilitychange'));

      // Give a tick for any async work
      await new Promise((r) => setTimeout(r, 10));
      expect(mockMarkAllStale).not.toHaveBeenCalled();
      expect(cacheStaleEvents).toHaveLength(0);
    });

    it('records timestamp when going hidden', () => {
      const before = Date.now();

      Object.defineProperty(document, 'visibilityState', {
        value: 'hidden',
        writable: true,
        configurable: true,
      });
      document.dispatchEvent(new Event('visibilitychange'));

      expect(_getLastActivityTimestamp()).toBeGreaterThanOrEqual(before);
      expect(mockMarkAllStale).not.toHaveBeenCalled();
    });
  });

  describe('pageshow event', () => {
    function makePageShowEvent(persisted: boolean): Event {
      const event = new Event('pageshow');
      Object.defineProperty(event, 'persisted', {
        value: persisted,
        configurable: true,
      });
      return event;
    }

    it('marks all cache entries stale when persisted (bfcache restore)', async () => {
      window.dispatchEvent(makePageShowEvent(true));

      await vi.waitFor(() => {
        expect(mockMarkAllStale).toHaveBeenCalledTimes(1);
      });
      expect(cacheStaleEvents).toHaveLength(1);
    });

    it('does NOT mark stale on a short hide before a persisted restore', async () => {
      // Simulate a brief tab switch: hidden a moment ago, well within threshold.
      _setHiddenAt(Date.now() - 2000);

      window.dispatchEvent(makePageShowEvent(true));

      await new Promise((r) => setTimeout(r, 10));
      expect(mockMarkAllStale).not.toHaveBeenCalled();
      expect(cacheStaleEvents).toHaveLength(0);
    });

    it('marks stale on a long hide before a persisted restore', async () => {
      _setHiddenAt(Date.now() - _STALE_THRESHOLD_MS - 1000);

      window.dispatchEvent(makePageShowEvent(true));

      await vi.waitFor(() => {
        expect(mockMarkAllStale).toHaveBeenCalledTimes(1);
      });
      expect(cacheStaleEvents).toHaveLength(1);
    });

    it('is a no-op when not persisted (initial navigation / reload)', async () => {
      window.dispatchEvent(makePageShowEvent(false));

      await new Promise((r) => setTimeout(r, 10));
      expect(mockMarkAllStale).not.toHaveBeenCalled();
      expect(cacheStaleEvents).toHaveLength(0);
    });

    it('updates lastActivityTimestamp on persisted restore', async () => {
      _setLastActivityTimestamp(0);
      const before = Date.now();
      window.dispatchEvent(makePageShowEvent(true));

      await vi.waitFor(() => {
        expect(mockMarkAllStale).toHaveBeenCalled();
      });
      expect(_getLastActivityTimestamp()).toBeGreaterThanOrEqual(before);
    });
  });

  describe('cleanup', () => {
    it('removes listeners on unlisten', async () => {
      unlisten();

      _setLastActivityTimestamp(0);
      document.dispatchEvent(new Event('resume'));
      Object.defineProperty(document, 'visibilityState', {
        value: 'visible',
        writable: true,
        configurable: true,
      });
      document.dispatchEvent(new Event('visibilitychange'));

      const pageShow = new Event('pageshow');
      Object.defineProperty(pageShow, 'persisted', {
        value: true,
        configurable: true,
      });
      window.dispatchEvent(pageShow);

      await new Promise((r) => setTimeout(r, 10));
      expect(mockMarkAllStale).not.toHaveBeenCalled();
      expect(cacheStaleEvents).toHaveLength(0);
    });
  });
});
