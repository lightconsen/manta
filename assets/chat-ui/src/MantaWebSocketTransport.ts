export interface WsRequest {
  id: string;
  method: string;
  params?: Record<string, unknown>;
}

export interface WsEvent {
  event: string;
  payload?: Record<string, unknown>;
  seq?: number;
}

export type EventCallback = (evt: WsEvent) => void;

/**
 * Manta WebSocket-native protocol client.
 *
 * 1. Opens a WebSocket to /ws
 * 2. Sends connect handshake on open
 * 3. Provides send() and event streaming for chat
 */
export class MantaWebSocketTransport {
  private ws: WebSocket | null = null;
  private reqId = 0;
  private sessionId: string;
  private reconnectDelay = 800;
  private readonly reconnectCap = 15000;
  private readonly reconnectMultiplier = 1.7;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private deviceId: string;
  private subscribedSessions: string[] = [];
  private listeners: Set<EventCallback> = new Set();

  constructor() {
    this.deviceId = localStorage.getItem("manta_device_id") || this.generateId();
    localStorage.setItem("manta_device_id", this.deviceId);

    const storedSession = localStorage.getItem("manta_session");
    this.sessionId = storedSession || `web:${this.deviceId}`;
    localStorage.setItem("manta_session", this.sessionId);

    this.connect();
  }

  onEvent(callback: EventCallback): () => void {
    this.listeners.add(callback);
    return () => this.listeners.delete(callback);
  }

  getSessionId(): string {
    return this.sessionId;
  }

  private generateId(): string {
    return "dev_" + Math.random().toString(36).slice(2, 10);
  }

  private connect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${location.host}/ws`;
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.reconnectDelay = 800;
      this.sendRequest("connect", {
        protocol_version: 1,
        client: { id: "web", version: "1.0.0" },
        device: { id: this.deviceId },
        scopes: ["chat", "read"],
      });
    };

    this.ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === "res" && msg.ok && msg.payload?.protocol_version) {
          if (this.subscribedSessions.length > 0) {
            this.sendRequest("sessions.subscribe", {
              session_ids: this.subscribedSessions,
            });
          }
        }
        if (msg.type === "event") {
          const evt = msg as WsEvent;
          if (evt.event === "session.created") {
            const sid = evt.payload?.session_id as string;
            if (sid && !this.subscribedSessions.includes(sid)) {
              this.subscribedSessions.push(sid);
            }
          }
          this.listeners.forEach((cb) => cb(evt));
        }
      } catch {
        /* ignore malformed */
      }
    };

    this.ws.onclose = () => {
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {
      this.scheduleReconnect();
    };
  }

  private scheduleReconnect() {
    if (this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, this.reconnectDelay);
    this.reconnectDelay = Math.min(
      this.reconnectDelay * this.reconnectMultiplier,
      this.reconnectCap
    );
  }

  private sendRequest(method: string, params?: Record<string, unknown>): string {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return "";
    const id = "req_" + ++this.reqId;
    this.ws.send(JSON.stringify({ type: "req", id, method, params }));
    return id;
  }

  createSession(): string {
    const newSessionId = `web:${this.deviceId}`;
    this.sessionId = newSessionId;
    localStorage.setItem("manta_session", this.sessionId);
    this.subscribedSessions = [];
    this.sendRequest("sessions.create", {
      session_id: this.sessionId,
    });
    return this.sessionId;
  }

  switchSession(sessionId: string): void {
    this.sessionId = sessionId;
    localStorage.setItem("manta_session", this.sessionId);
    if (!this.subscribedSessions.includes(sessionId)) {
      this.subscribedSessions.push(sessionId);
      this.sendRequest("sessions.subscribe", {
        session_ids: [sessionId],
      });
    }
  }

  sendMessage(text: string): void {
    this.sendRequest("chat.send", {
      session_id: this.sessionId,
      message: text,
    });
  }

}
