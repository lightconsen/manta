import type {
  ChatModelAdapter,
  ChatModelRunOptions,
  ChatModelRunResult,
  TextMessagePart,
  ReasoningMessagePart,
  ToolCallMessagePart,
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

export type EventCallback = (evt: WsEvent) => void;

function makeTextPart(text: string): TextMessagePart {
  return { type: "text", text };
}

function makeReasoningPart(text: string): ReasoningMessagePart {
  return { type: "reasoning", text };
}

function makeToolCallPart(
  toolCallId: string,
  toolName: string,
  args: Record<string, unknown>,
  result?: unknown
): ToolCallMessagePart {
  return {
    type: "tool-call",
    toolCallId,
    toolName,
    args,
    argsText: JSON.stringify(args),
    result,
  } as ToolCallMessagePart;
}

/**
 * Manta WebSocket-native protocol client implementing ChatModelAdapter.
 *
 * Uses AsyncGenerator to stream updates for assistant-ui consumption.
 */
export class MantaWebSocketTransport implements ChatModelAdapter {
  private ws: WebSocket | null = null;
  private reqId = 0;
  private sessionId: string;
  private eventQueue: WsEvent[] = [];
  private eventWaiters: Array<(evt: WsEvent | null) => void> = [];
  private reconnectDelay = 800;
  private readonly reconnectCap = 15000;
  private readonly reconnectMultiplier = 1.7;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private deviceId: string;
  private subscribedSessions: string[] = [];
  private listeners: Set<EventCallback> = new Set();

  constructor() {
    this.deviceId =
      localStorage.getItem("manta_device_id") || this.generateId();
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
        if (
          msg.type === "res" &&
          msg.ok &&
          msg.payload?.protocol_version
        ) {
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

  private sendRequest(
    method: string,
    params?: Record<string, unknown>
  ): string {
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

  async *run(
    options: ChatModelRunOptions
  ): AsyncGenerator<ChatModelRunResult, void> {
    const { messages, abortSignal } = options;
    const last = messages[messages.length - 1];
    if (!last || last.role !== "user") {
      return;
    }

    const text = last.content
      .map((c) => (c.type === "text" ? c.text : ""))
      .join("");

    this.sendRequest("chat.send", {
      session_id: this.sessionId,
      message: text,
    });

    const parts: (
      | TextMessagePart
      | ReasoningMessagePart
      | ToolCallMessagePart
    )[] = [];
    let currentText = "";
    let currentReasoning = "";
    let toolCalls = new Map<string, ToolCallMessagePart>();
    let aborted = false;

    try {
      while (true) {
        const evt = await this.nextEvent(abortSignal);
        if (!evt) {
          aborted = true;
          break;
        }

        switch (evt.event) {
          case "chat.delta": {
            const delta = (evt.payload?.content as string) || "";
            currentText += delta;
            // Rebuild parts: reasoning (if any) + text + tool calls
            const newParts: typeof parts = [];
            if (currentReasoning) {
              newParts.push(makeReasoningPart(currentReasoning));
            }
            for (const tc of toolCalls.values()) {
              newParts.push(tc);
            }
            newParts.push(makeTextPart(currentText));
            parts.length = 0;
            parts.push(...newParts);
            yield { content: [...parts] };
            break;
          }
          case "agent.thinking": {
            const thinking = (evt.payload?.content as string) || "";
            currentReasoning += thinking;
            const newParts: typeof parts = [];
            if (currentReasoning) {
              newParts.push(makeReasoningPart(currentReasoning));
            }
            for (const tc of toolCalls.values()) {
              newParts.push(tc);
            }
            if (currentText) {
              newParts.push(makeTextPart(currentText));
            }
            parts.length = 0;
            parts.push(...newParts);
            yield { content: [...parts] };
            break;
          }
          case "tool.calling": {
            const toolName = (evt.payload?.tool_name as string) || "tool";
            const toolArgs =
              (evt.payload?.arguments as Record<string, unknown>) || {};
            const toolCallId = `tc_${toolCalls.size}_${Date.now()}`;
            const tc = makeToolCallPart(toolCallId, toolName, toolArgs);
            toolCalls.set(toolCallId, tc);
            const newParts: typeof parts = [];
            if (currentReasoning) {
              newParts.push(makeReasoningPart(currentReasoning));
            }
            for (const t of toolCalls.values()) {
              newParts.push(t);
            }
            if (currentText) {
              newParts.push(makeTextPart(currentText));
            }
            parts.length = 0;
            parts.push(...newParts);
            yield { content: [...parts] };
            break;
          }
          case "tool.result": {
            const toolName = (evt.payload?.tool_name as string) || "";
            const result = evt.payload?.result;
            // Find matching tool call by name and update with result
            for (const [id, tc] of toolCalls) {
              if (tc.toolName === toolName && tc.result === undefined) {
                const updated = makeToolCallPart(
                  id,
                  toolName,
                  tc.args,
                  result
                );
                toolCalls.set(id, updated);
                break;
              }
            }
            const newParts: typeof parts = [];
            if (currentReasoning) {
              newParts.push(makeReasoningPart(currentReasoning));
            }
            for (const t of toolCalls.values()) {
              newParts.push(t);
            }
            if (currentText) {
              newParts.push(makeTextPart(currentText));
            }
            parts.length = 0;
            parts.push(...newParts);
            yield { content: [...parts] };
            break;
          }
          case "chat.final": {
            const response =
              (evt.payload?.response as string) || currentText;
            currentText = response;
            const finalParts: typeof parts = [];
            if (currentReasoning) {
              finalParts.push(makeReasoningPart(currentReasoning));
            }
            for (const t of toolCalls.values()) {
              finalParts.push(t);
            }
            finalParts.push(makeTextPart(currentText));
            yield { content: finalParts, status: { type: "complete", reason: "stop" } };
            return;
          }
          case "chat.error": {
            throw new Error(
              (evt.payload?.message as string) || "Chat error"
            );
          }
        }
      }
    } finally {
      if (aborted) {
        this.sendRequest("chat.abort", {
          session_id: this.sessionId,
        });
      }
    }
  }

  private nextEvent(abortSignal?: AbortSignal): Promise<WsEvent | null> {
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
        if (abortSignal) {
          abortSignal.removeEventListener("abort", onAbort);
        }
        const idx = this.eventWaiters.indexOf(waiter);
        if (idx >= 0) this.eventWaiters.splice(idx, 1);
      };

      if (abortSignal?.aborted) {
        onAbort();
        return;
      }
      if (abortSignal) {
        abortSignal.addEventListener("abort", onAbort);
      }
    });
  }
}
