import { WebSocketConfig, WebSocketState, MessageData } from '../types';

export class WebSocketManager {
  private ws: WebSocket | null = null;
  private config: WebSocketConfig;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 5;
  private reconnectTimeout: NodeJS.Timeout | null = null;
  private conversationId: string | null = null;

  constructor(config: WebSocketConfig) {
    this.config = config;
  }

  setConversationId(id: string | null): void {
    this.conversationId = id;
  }

  getConversationId(): string | null {
    return this.conversationId;
  }

  connect(conversationId?: string): void {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    let url = `${protocol}//${window.location.host}/ws`;
    if (conversationId) {
      url += `?conversation=${encodeURIComponent(conversationId)}`;
    }

    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      console.log('WebSocket connected');
      this.reconnectAttempts = 0;
      this.config.onStateChange(WebSocketState.Connected);
    };

    this.ws.onclose = () => {
      console.log('WebSocket closed');
      this.config.onStateChange(WebSocketState.Disconnected);

      if (this.reconnectAttempts < this.maxReconnectAttempts) {
        this.reconnectAttempts++;
        this.reconnectTimeout = setTimeout(() => {
          this.connect(this.conversationId || undefined);
        }, 2000);
      }
    };

    this.ws.onerror = (err) => {
      console.error('WebSocket error:', err);
      this.config.onError?.(err);
    };

    this.ws.onmessage = (event) => {
      try {
        const data: MessageData = JSON.parse(event.data);
        this.config.onMessage(data);
      } catch (err) {
        console.error('Failed to parse message:', err);
      }
    };
  }

  disconnect(): void {
    if (this.reconnectTimeout) {
      clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }
    this.ws?.close();
    this.ws = null;
  }

  send(message: string): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(message);
    }
  }

  isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }
}
