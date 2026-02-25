import { useEffect, useRef } from 'react';
import { API_BASE } from '../api';
import type { SSEEvent } from '../types';

export function useSSE(onEvent: (event: SSEEvent) => void, onReconnect?: () => void) {
  const onEventRef = useRef(onEvent);
  onEventRef.current = onEvent;
  const onReconnectRef = useRef(onReconnect);
  onReconnectRef.current = onReconnect;

  useEffect(() => {
    let es: EventSource | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

    function connect() {
      es = new EventSource(`${API_BASE}/events`);

      es.onopen = () => {
        onReconnectRef.current?.();
      };

      es.addEventListener('change', (e) => {
        try {
          const data: SSEEvent = JSON.parse(e.data);
          onEventRef.current(data);
        } catch {
          // ignore malformed events
        }
      });

      es.onerror = () => {
        es?.close();
        es = null;
        // Reconnect after 2s
        reconnectTimer = setTimeout(connect, 2000);
      };
    }

    function handleVisibility() {
      if (document.hidden) {
        es?.close();
        es = null;
        if (reconnectTimer) {
          clearTimeout(reconnectTimer);
          reconnectTimer = null;
        }
      } else if (!es) {
        connect();
      }
    }

    connect();
    document.addEventListener('visibilitychange', handleVisibility);

    return () => {
      es?.close();
      if (reconnectTimer) clearTimeout(reconnectTimer);
      document.removeEventListener('visibilitychange', handleVisibility);
    };
  }, []);
}
