import { ref, onUnmounted } from 'vue';

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

export function useChatWebSocket(
  sessionId: string,
  onMessage: (data: string) => void
) {
  const ws = ref<WebSocket | null>(null);
  const isConnected = ref(false);
  const isReconnecting = ref(false);
  const showReconnectButton = ref(false);
  const reconnectAttempts = ref(0);

  const maxReconnectAttempts = 5;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  const connect = () => {
    if (ws.value) {
      ws.value.close();
    }

    const wsUrl = `ws://localhost:3000/ws?user_id=${encodeURIComponent(sessionId)}`;
    ws.value = new WebSocket(wsUrl);

    ws.value.onopen = () => {
      isConnected.value = true;
      reconnectAttempts.value = 0;
      showReconnectButton.value = false;
      isReconnecting.value = false;
    };

    ws.value.onmessage = (event) => {
      onMessage(event.data);
    };

    ws.value.onclose = () => {
      isConnected.value = false;
      attemptReconnect();
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

  const send = (data: string) => {
    if (ws.value?.readyState === WebSocket.OPEN) {
      ws.value.send(data);
    }
  };

  const close = () => {
    if (ws.value) {
      ws.value.close();
    }
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
    }
  };

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
