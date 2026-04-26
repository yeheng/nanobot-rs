import { onUnmounted, ref, watch, type Ref } from 'vue';

export interface WebSocketStatus {
  isConnected: boolean;
  isReconnecting: boolean;
  showReconnectButton: boolean;
  reconnectAttempts: number;
}

export interface WebSocketMessage {
  type: string;
  content?: string;
  name?: string;
  arguments?: string;
  output?: string;
  error?: string;
  message?: string;
}

export function useIMWebSocket(
  chatId: Ref<string>,
  onMessage: (data: string) => void
) {
  const ws = ref<WebSocket | null>(null);
  const isConnected = ref(false);
  const isReconnecting = ref(false);
  const showReconnectButton = ref(false);
  const reconnectAttempts = ref(0);

  const maxReconnectAttempts = 5;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let isManualClose = false;

  const connect = () => {
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }

    if (ws.value) {
      isManualClose = true;
      ws.value.close();
      // NOTE: do NOT reset isManualClose here.
      // onclose is async; resetting it immediately causes the old socket's
      // onclose to fire with isManualClose=false, triggering a spurious
      // reconnect that races with the new socket.
    }

    const wsUrl = `${import.meta.env.VITE_WS_URL || 'ws://localhost:3000'}/ws?user_id=${encodeURIComponent(chatId.value)}`;
    ws.value = new WebSocket(wsUrl);

    ws.value.onopen = () => {
      isConnected.value = true;
      reconnectAttempts.value = 0;
      showReconnectButton.value = false;
      isReconnecting.value = false;
      isManualClose = false;
    };

    ws.value.onmessage = (event) => {
      onMessage(event.data);
    };

    ws.value.onclose = () => {
      isConnected.value = false;
      if (!isManualClose) {
        attemptReconnect();
      }
    };

    ws.value.onerror = () => {
      isConnected.value = false;
    };
  };

  const attemptReconnect = () => {
    if (reconnectAttempts.value >= maxReconnectAttempts) {
      showReconnectButton.value = true;
      isReconnecting.value = false;
      return;
    }

    isReconnecting.value = true;
    const delay = Math.min(1000 * Math.pow(2, reconnectAttempts.value), 30000);
    reconnectAttempts.value++;

    reconnectTimer = setTimeout(() => {
      connect();
    }, delay);
  };

  const manualReconnect = () => {
    reconnectAttempts.value = 0;
    showReconnectButton.value = false;
    isReconnecting.value = true;
    connect();
  };

  const send = (data: string): boolean => {
    if (ws.value?.readyState === WebSocket.OPEN) {
      ws.value.send(data);
      return true;
    }
    return false;
  };

  const close = () => {
    if (ws.value) {
      isManualClose = true;
      ws.value.close();
    }
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
  };

  watch(chatId, () => {
    connect();
  });

  onUnmounted(() => {
    close();
  });

  return {
    ws,
    isConnected,
    isReconnecting,
    showReconnectButton,
    reconnectAttempts,
    connect,
    manualReconnect,
    send,
    close
  };
}
