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
});
