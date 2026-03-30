import { useEffect } from 'react';
import { useQueryClient } from '@tanstack/react-query';

interface StatusChangedMessage {
  type: 'server.status_changed';
  serverId: string;
}

/**
 * Opens a WebSocket connection that listens for real-time server status change
 * events and invalidates the `['servers', userId]` query on each event.
 *
 * Silently falls back to the query's polling mechanism on any WebSocket failure —
 * no error is surfaced to the user.
 */
export function useServerStatusSocket(userId: string | null | undefined): void {
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!userId) return;

    let ws: WebSocket | null = null;

    try {
      const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      const wsUrl = `${protocol}//${window.location.host}/api/v1/ws/status`;
      ws = new WebSocket(wsUrl);

      ws.onmessage = (event: MessageEvent<string>) => {
        try {
          const msg = JSON.parse(event.data) as StatusChangedMessage;
          if (msg.type === 'server.status_changed') {
            void queryClient.invalidateQueries({
              queryKey: ['servers', userId],
            });
          }
        } catch {
          // Ignore unparseable messages
        }
      };

      ws.onerror = () => {
        // Silently ignore — polling covers us
      };
    } catch {
      // Silently ignore connection failure — polling covers us
    }

    return () => {
      ws?.close();
    };
  }, [userId, queryClient]);
}
