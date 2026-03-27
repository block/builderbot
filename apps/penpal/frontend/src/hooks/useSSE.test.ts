import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useSSE } from './useSSE';

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  listeners: Record<string, ((e: { data: string }) => void)[]> = {};
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(type: string, fn: (e: { data: string }) => void) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(fn);
  }

  close() {
    this.closed = true;
  }

  emit(type: string, data: string) {
    this.listeners[type]?.forEach((fn) => fn({ data }));
  }

  simulateOpen() {
    this.onopen?.();
  }
}

beforeEach(() => {
  MockEventSource.instances = [];
  vi.stubGlobal('EventSource', MockEventSource);
});

afterEach(() => {
  vi.restoreAllMocks();
});

// E-PENPAL-SSE-RECONNECT: verifies SSE connection, reconnect on error, and onReconnect callback.
describe('useSSE', () => {
  it('connects to /events and calls handler on change events', () => {
    const handler = vi.fn();
    renderHook(() => useSSE(handler));

    expect(MockEventSource.instances).toHaveLength(1);
    expect(MockEventSource.instances[0].url).toContain('/events');

    act(() => {
      MockEventSource.instances[0].emit('change', JSON.stringify({ type: 'files', project: 'p' }));
    });

    expect(handler).toHaveBeenCalledWith({ type: 'files', project: 'p' });
  });

  it('closes connection on unmount', () => {
    const handler = vi.fn();
    const { unmount } = renderHook(() => useSSE(handler));

    const es = MockEventSource.instances[0];
    expect(es.closed).toBe(false);

    unmount();
    expect(es.closed).toBe(true);
  });

  it('ignores malformed event data', () => {
    const handler = vi.fn();
    renderHook(() => useSSE(handler));

    act(() => {
      MockEventSource.instances[0].emit('change', 'not json');
    });

    expect(handler).not.toHaveBeenCalled();
  });

  it('calls onReconnect when connection opens', () => {
    const handler = vi.fn();
    const onReconnect = vi.fn();
    renderHook(() => useSSE(handler, onReconnect));

    act(() => {
      MockEventSource.instances[0].simulateOpen();
    });

    expect(onReconnect).toHaveBeenCalledTimes(1);
  });

  it('calls onReconnect again after reconnect', () => {
    vi.useFakeTimers();
    const handler = vi.fn();
    const onReconnect = vi.fn();
    renderHook(() => useSSE(handler, onReconnect));

    // First open
    act(() => {
      MockEventSource.instances[0].simulateOpen();
    });
    expect(onReconnect).toHaveBeenCalledTimes(1);

    // Simulate error → reconnect
    act(() => {
      MockEventSource.instances[0].onerror?.();
    });
    act(() => {
      vi.advanceTimersByTime(2000);
    });

    // Second connection should have been created
    expect(MockEventSource.instances).toHaveLength(2);

    // Simulate second open
    act(() => {
      MockEventSource.instances[1].simulateOpen();
    });
    expect(onReconnect).toHaveBeenCalledTimes(2);

    vi.useRealTimers();
  });

  it('does not error when onReconnect is not provided', () => {
    const handler = vi.fn();
    renderHook(() => useSSE(handler));

    // Should not throw
    act(() => {
      MockEventSource.instances[0].simulateOpen();
    });
  });

  // E-PENPAL-SSE-RECONNECT: verifies connection is closed when tab becomes hidden.
  it('closes connection when tab becomes hidden', () => {
    const handler = vi.fn();
    renderHook(() => useSSE(handler));

    const es = MockEventSource.instances[0];
    expect(es.closed).toBe(false);

    // Simulate tab going hidden
    Object.defineProperty(document, 'hidden', { value: true, writable: true, configurable: true });
    act(() => {
      document.dispatchEvent(new Event('visibilitychange'));
    });

    expect(es.closed).toBe(true);

    // Restore
    Object.defineProperty(document, 'hidden', { value: false, writable: true, configurable: true });
  });

  // E-PENPAL-SSE-RECONNECT: verifies reconnect when tab becomes visible after being hidden.
  it('reconnects when tab becomes visible again', () => {
    const handler = vi.fn();
    const onReconnect = vi.fn();
    renderHook(() => useSSE(handler, onReconnect));

    expect(MockEventSource.instances).toHaveLength(1);

    // Tab goes hidden → closes connection
    Object.defineProperty(document, 'hidden', { value: true, writable: true, configurable: true });
    act(() => {
      document.dispatchEvent(new Event('visibilitychange'));
    });

    expect(MockEventSource.instances[0].closed).toBe(true);

    // Tab becomes visible again → reconnects
    Object.defineProperty(document, 'hidden', { value: false, writable: true, configurable: true });
    act(() => {
      document.dispatchEvent(new Event('visibilitychange'));
    });

    // A new EventSource should have been created
    expect(MockEventSource.instances).toHaveLength(2);
    expect(MockEventSource.instances[1].closed).toBe(false);

    // Restore
    Object.defineProperty(document, 'hidden', { value: false, writable: true, configurable: true });
  });

  // E-PENPAL-SSE-RECONNECT: verifies pending reconnect timer is cancelled when tab goes hidden.
  it('cancels pending reconnect timer when tab becomes hidden', () => {
    vi.useFakeTimers();
    const handler = vi.fn();
    renderHook(() => useSSE(handler));

    // Trigger error to start reconnect timer
    act(() => {
      MockEventSource.instances[0].onerror?.();
    });

    // Before timer fires, tab goes hidden
    Object.defineProperty(document, 'hidden', { value: true, writable: true, configurable: true });
    act(() => {
      document.dispatchEvent(new Event('visibilitychange'));
    });

    // Advance past reconnect delay — should NOT create new connection
    act(() => {
      vi.advanceTimersByTime(3000);
    });

    // Only the original instance should exist (no reconnect fired)
    expect(MockEventSource.instances).toHaveLength(1);

    // Restore
    Object.defineProperty(document, 'hidden', { value: false, writable: true, configurable: true });
    vi.useRealTimers();
  });
});
