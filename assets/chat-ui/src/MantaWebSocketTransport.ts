import type {
  ChatModelAdapter,
  ChatModelRunResult,
  ThreadMessage,
} from "@assistant-ui/react";

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

/**
 * Custom ChatModelAdapter that speaks the Manta WebSocket-native protocol.
 *
 * 1. Opens a WebSocket to /ws
 * 2. Sends connect handshake on open
 * 3. Maps assistant-ui message stream to chat.send + chat.delta/chat.final
 */
export class MantaWebSocketTransport implements ChatModelAdapter {
  private ws: WebSocket | null = null;
  private reqId = 0;
  private connected = false;
  private sessionId: string;
  private eventQueue: WsEvent[] = [];
  private eventWaiters: Array<(evt: WsEvent | null) => void> = [];
  private reconnectDelay = 800;
  private readonly reconnectCap = 15000;
  private readonly reconnectMultiplier = 1.7;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private deviceId: string;
  private subscribedSessions: string[] = [];

  constructor() {
    this.sessionId = localStorage.getItem("manta_session") || this.generateId();
    localStorage.setItem("manta_session", this.sessionId);
    this.deviceId = localStorage.getItem("manta_device_id") || this.generateId();
    localStorage.setItem("manta_device_id", this.deviceId);
    this.connect();
  }

  private generateId(): string {
    return "sess_" + Math.random().toString(36).slice(2, 10);
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
          this.connected = true;
          // Re-subscribe to previous sessions after reconnect
          if (this.subscribedSessions.length > 0) {
            this.sendRequest("sessions.subscribe", {
              session_ids: this.subscribedSessions,
            });
          }
        }
        if (msg.type === "event") {
          const evt = msg as WsEvent;
          // Track auto-created sessions
          if (evt.event === "session.created") {
            const sid = evt.payload?.session_id as string;
            if (sid && !this.subscribedSessions.includes(sid)) {
              this.subscribedSessions.push(sid);
            }
          }
          if (this.eventWaiters.length > 0) {
            const waiter = this.eventWaiters.shift()!;
            waiter(evt);
          } else {
            this.eventQueue.push(evt);
          }
        }
      } catch {
        /* ignore malformed */
      }
    };

    this.ws.onclose = () => {
      this.connected = false;
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {
      this.connected = false;
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

  async *run(options: {
    messages: ThreadMessage[];
    abortSignal: AbortSignal;
  }): AsyncGenerator<ChatModelRunResult> {
    const { messages } = options;
    const last = messages[messages.length - 1];
    if (!last || last.role !== "user") return;

    const text = last.content
      .map((c) => (c.type === "text" ? c.text : ""))
      .join("");

    this.sendRequest("chat.send", {
      session_id: this.sessionId,
      message: text,
    });

    let buffer = "";

    while (true) {
      const evt = await this.nextEvent(options.abortSignal);
      if (!evt) break;

      switch (evt.event) {
        case "chat.delta": {
          const delta = (evt.payload?.content as string) || "";
          buffer += delta;
          yield { content: [{ type: "text" as const, text: buffer }] };
          break;
        }
        case "chat.final": {
          buffer = (evt.payload?.response as string) || buffer;
          yield { content: [{ type: "text" as const, text: buffer }] };
          return;
        }
        case "chat.error": {
          throw new Error((evt.payload?.message as string) || "Chat error");
        }
        case "agent.thinking":
        case "tool.calling":
        case "tool.result": {
          // Assistant-UI shows these as status/tool-call UI elements
          yield {
            content: [{ type: "text" as const, text: buffer }],
            metadata: {
              thinking:
                evt.event === "agent.thinking"
                  ? ((evt.payload?.content as string) ?? "")
                  : undefined,
              toolCall:
                evt.event === "tool.calling"
                  ? {
                      name: (evt.payload?.tool_name as string) ?? "",
                      args: (evt.payload?.arguments as Record<string, unknown>) ?? {},
                    }
                  : undefined,
            },
          };
          break;
        }
      }
    }
  }

  private nextEvent(abortSignal: AbortSignal): Promise<WsEvent | null> {
    return new Promise((resolve) => {
      if (this.eventQueue.length > 0) {
        resolve(this.eventQueue.shift()!);
        return;
      }

      const waiter = (evt: WsEvent | null) => {
        cleanup();
        resolve(evt);
      };
      this.eventWaiters.push(waiter);

      const onAbort = () => {
        cleanup();
        resolve(null);
      };

      const cleanup = () => {
        abortSignal.removeEventListener("abort", onAbort);
        const idx = this.eventWaiters.indexOf(waiter);
        if (idx >= 0) this.eventWaiters.splice(idx, 1);
      };

      if (abortSignal.aborted) {
        onAbort();
        return;
      }
      abortSignal.addEventListener("abort", onAbort);
    });
  }
}
