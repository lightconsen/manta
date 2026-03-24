import { SSEConfig, ConnectionState, MessageData } from '../types';

/**
 * SSE Manager for Manta Web Terminal
 *
 * Handles Server-Sent Events connection for receiving messages from the server
 * and HTTP POST for sending messages. This replaces WebSocket for a more
 * reliable streaming experience that works through proxies and HTTP/1.1.
 */
export class SSEManager {
  private eventSource: EventSource | null = null;
  private config: SSEConfig;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 5;
  private reconnectTimeout: NodeJS.Timeout | null = null;
  private conversationId: string | null = null;
  private messageQueue: string[] = [];

  constructor(config: SSEConfig) {
    this.config = config;
  }

  setConversationId(id: string | null): void {
    this.conversationId = id;
  }

  getConversationId(): string | null {
    return this.conversationId;
  }

  /**
   * Connect to the SSE endpoint
   */
  connect(conversationId?: string): void {
    if (conversationId) {
      this.conversationId = conversationId;
    }

    const url = `${window.location.protocol}//${window.location.host}/api/events`;

    console.log('[SSE] Connecting to:', url);

    try {
      this.eventSource = new EventSource(url);

      this.eventSource.onopen = () => {
        console.log('[SSE] Connected');
        this.reconnectAttempts = 0;
        this.config.onStateChange(ConnectionState.Connected);

        // Process any queued messages
        while (this.messageQueue.length > 0) {
          const msg = this.messageQueue.shift();
          if (msg) this.send(msg);
        }
      };

      this.eventSource.onmessage = (event) => {
        try {
          const data: MessageData = JSON.parse(event.data);
          console.log('[SSE] Message received:', data.type);
          this.config.onMessage(data);
        } catch (err) {
          console.error('[SSE] Failed to parse message:', err);
        }
      };

      this.eventSource.onerror = (err) => {
        console.error('[SSE] Error:', err);
        this.config.onStateChange(ConnectionState.Error);

        if (this.eventSource?.readyState === EventSource.CLOSED) {
          this.handleReconnect();
        }
      };
    } catch (err) {
      console.error('[SSE] Failed to connect:', err);
      this.config.onError?.(err as Event);
      this.handleReconnect();
    }
  }

  private handleReconnect(): void {
    if (this.reconnectAttempts < this.maxReconnectAttempts) {
      this.reconnectAttempts++;
      console.log(`[SSE] Reconnecting... attempt ${this.reconnectAttempts}/${this.maxReconnectAttempts}`);
      this.config.onStateChange(ConnectionState.Reconnecting);

      this.reconnectTimeout = setTimeout(() => {
        this.connect(this.conversationId || undefined);
      }, 2000);
    } else {
      console.error('[SSE] Max reconnect attempts reached');
      this.config.onStateChange(ConnectionState.Disconnected);
    }
  }

  /**
   * Disconnect from the SSE endpoint
   */
  disconnect(): void {
    if (this.reconnectTimeout) {
      clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }

    if (this.eventSource) {
      this.eventSource.close();
      this.eventSource = null;
    }

    console.log('[SSE] Disconnected');
    this.config.onStateChange(ConnectionState.Disconnected);
  }

  /**
   * Send a message via HTTP POST
   */
  async send(message: string): Promise<void> {
    if (!this.isConnected() && !navigator.onLine) {
      console.warn('[SSE] Offline, queuing message');
      this.messageQueue.push(message);
      return;
    }

    const url = `${window.location.protocol}//${window.location.host}/api/chat`;

    const payload = {
      message,
      conversation_id: this.conversationId,
      user_id: 'web_user',
    };

    try {
      const response = await fetch(url, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(payload),
      });

      if (!response.ok) {
        const errorData = await response.json().catch(() => ({ error: 'Unknown error' }));
        throw new Error(errorData.error || `HTTP ${response.status}`);
      }

      const data = await response.json();
      console.log('[SSE] Message sent, conversation_id:', data.conversation_id);

      // Update conversation ID if this is a new conversation
      if (data.conversation_id && !this.conversationId) {
        this.conversationId = data.conversation_id;
        this.config.onConversationId?.(data.conversation_id);
      }
    } catch (err) {
      console.error('[SSE] Failed to send message:', err);
      this.config.onError?.(err as Event);
      throw err;
    }
  }

  /**
   * Check if connected
   */
  isConnected(): boolean {
    return this.eventSource?.readyState === EventSource.OPEN;
  }

  /**
   * Get connection state
   */
  getConnectionState(): ConnectionState {
    if (!this.eventSource) {
      return ConnectionState.Disconnected;
    }

    switch (this.eventSource.readyState) {
      case EventSource.CONNECTING:
        return ConnectionState.Connecting;
      case EventSource.OPEN:
        return ConnectionState.Connected;
      case EventSource.CLOSED:
        return ConnectionState.Disconnected;
      default:
        return ConnectionState.Disconnected;
    }
  }
}
