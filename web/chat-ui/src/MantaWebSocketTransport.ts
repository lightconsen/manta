import type {
  ChatModelAdapter,
  ChatModelRunOptions,
  ChatModelRunResult,
  TextMessagePart,
  ReasoningMessagePart,
  ToolCallMessagePart,
} from "@assistant-ui/react";

import {
  parseCommand,
  findCommand,
  LOCAL_COMMANDS,
} from "./slash-commands";

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
export type NetworkStatus = "connected" | "disconnected" | "connecting";
export type StatusCallback = (status: NetworkStatus) => void;

export interface ChatMessagePart {
  type: string;
  text?: string;
  toolName?: string;
  args?: Record<string, unknown>;
  result?: unknown;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  parts?: ChatMessagePart[];
  timestamp?: number;
  /** Metadata: how many tools were called */
  toolCount?: number;
  /** Metadata: how long the response took (ms) */
  durationMs?: number;
  /** Live streaming status — set during response, cleared on complete */
  liveStatus?: {
    status: "thinking" | "tool_calling";
    toolName?: string;
  };
}

export type MessagesCallback = (messages: ChatMessage[]) => void;
export type SessionCallback = () => void;

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

/** Convert assistant-ui part to ChatMessagePart preserving tool call metadata. */
function toChatPart(
  p: TextMessagePart | ReasoningMessagePart | ToolCallMessagePart
): ChatMessagePart {
  if (p.type === "tool-call") {
    return {
      type: p.type,
      toolName: p.toolName,
      args: p.args,
      result: p.result,
    };
  }
  return { type: p.type, text: (p as any).text || "" };
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
  private statusListeners: Set<StatusCallback> = new Set();
  private responseWaiters = new Map<
    string,
    { resolve: (v: unknown) => void; reject: (e: Error) => void }
  >();
  private currentStatus: NetworkStatus = "connecting";
  private messages: ChatMessage[] = [];
  private messagesListeners: Set<MessagesCallback> = new Set();
  private sessionListeners: Set<SessionCallback> = new Set();
  private isRunningFlag = false;
  private runListeners: Set<(running: boolean) => void> = new Set();
  private currentAbortController: AbortController | null = null;
  private serverInfo: { version?: string; conn_id?: string; features?: string[]; scopes_granted?: string[] } = {};
  private gatewayUrl: string = "";

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

  onStatusChange(callback: StatusCallback): () => void {
    this.statusListeners.add(callback);
    callback(this.currentStatus);
    return () => this.statusListeners.delete(callback);
  }

  onSessionChange(callback: SessionCallback): () => void {
    this.sessionListeners.add(callback);
    return () => this.sessionListeners.delete(callback);
  }

  onRunStateChange(callback: (running: boolean) => void): () => void {
    this.runListeners.add(callback);
    callback(this.isRunningFlag);
    return () => this.runListeners.delete(callback);
  }

  abort(): void {
    this.currentAbortController?.abort();
    this.sendRequest("chat.abort", {
      session_id: this.sessionId,
    });
  }

  private notifySessionChange() {
    this.sessionListeners.forEach((cb) => cb());
  }

  getSessionId(): string {
    return this.sessionId;
  }

  private setStatus(status: NetworkStatus) {
    if (this.currentStatus === status) return;
    this.currentStatus = status;
    this.statusListeners.forEach((cb) => cb(status));
  }

  private generateId(): string {
    return "dev_" + Math.random().toString(36).slice(2, 10);
  }

  private connect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }

    this.setStatus("connecting");
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${location.host}/ws`;
    this.gatewayUrl = url;
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.reconnectDelay = 800;
      this.sendRequest("connect", {
        protocol_version: 1,
        client: { id: "web", version: "1.0.0" },
        device: { id: this.deviceId },
        scopes: ["chat", "read", "write"],
      });
    };

    this.ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === "res") {
          // Resolve response waiters
          const waiter = this.responseWaiters.get(msg.id);
          if (waiter) {
            this.responseWaiters.delete(msg.id);
            if (msg.ok) {
              waiter.resolve(msg.payload);
            } else {
              waiter.reject(
                new Error(msg.error?.message || "Request failed")
              );
            }
          }
          if (msg.ok && msg.payload?.protocol_version) {
            this.setStatus("connected");
            this.serverInfo = {
              version: msg.payload.server?.version,
              conn_id: msg.payload.server?.conn_id,
              features: msg.payload.features,
              scopes_granted: msg.payload.scopes_granted,
            };
            if (this.subscribedSessions.length > 0) {
              this.sendRequest("sessions.subscribe", {
                session_ids: this.subscribedSessions,
              });
            }
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
      this.setStatus("disconnected");
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {
      this.setStatus("disconnected");
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
    // Reuse an existing empty session if one exists
    const sessions = this.getLocalSessions();
    for (const sid of sessions) {
      if (this.getHistory(sid).length === 0) {
        this.sessionId = sid;
        localStorage.setItem("manta_session", this.sessionId);
        this.subscribedSessions = [];
        this.sendRequest("sessions.create", {
          session_id: this.sessionId,
        });
        this.notifySessionChange();
        return this.sessionId;
      }
    }

    const newSessionId = `web:${this.deviceId}_${Date.now()}`;
    this.sessionId = newSessionId;
    localStorage.setItem("manta_session", this.sessionId);
    this.subscribedSessions = [];
    this.sendRequest("sessions.create", {
      session_id: this.sessionId,
    });
    // Persist session in local list
    if (!sessions.includes(newSessionId)) {
      sessions.unshift(newSessionId);
      localStorage.setItem("manta_sessions", JSON.stringify(sessions));
    }
    this.notifySessionChange();
    return this.sessionId;
  }

  getLocalSessions(): string[] {
    try {
      return JSON.parse(localStorage.getItem("manta_sessions") || "[]");
    } catch {
      return [];
    }
  }

  async listSessions(): Promise<Array<{ id: string; label?: string }>> {
    const local = this.getLocalSessions();
    if (this.currentStatus !== "connected") {
      return local.map((id) => ({ id, label: this.sessionLabel(id) }));
    }
    try {
      const payload = (await this.sendRequestAndWait("sessions.list", {})) as
        | { sessions?: Array<{ session_id: string; name?: string }> }
        | undefined;
      const remote = payload?.sessions || [];
      const merged = new Map<string, string>();
      for (const s of remote) {
        merged.set(s.session_id, s.name || this.sessionLabel(s.session_id));
      }
      for (const id of local) {
        if (!merged.has(id)) merged.set(id, this.sessionLabel(id));
      }
      return Array.from(merged.entries()).map(([id, label]) => ({
        id,
        label,
      }));
    } catch {
      return local.map((id) => ({ id, label: this.sessionLabel(id) }));
    }
  }

  private sessionLabel(id: string): string {
    const parts = id.split("_");
    const last = parts[parts.length - 1];
    if (/^\d+$/.test(last)) {
      const d = new Date(parseInt(last));
      if (!isNaN(d.getTime())) {
        return d.toLocaleString(undefined, {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        });
      }
    }
    return id.slice(0, 20);
  }

  private waitForConnected(timeout = 8000): Promise<void> {
    return new Promise((resolve, reject) => {
      if (this.currentStatus === "connected") {
        resolve();
        return;
      }
      const unsub = this.onStatusChange((status) => {
        if (status === "connected") {
          unsub();
          resolve();
        }
      });
      setTimeout(() => {
        unsub();
        reject(new Error("WebSocket connection timeout"));
      }, timeout);
    });
  }

  private sendRequestAndWait(
    method: string,
    params?: Record<string, unknown>
  ): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const id = this.sendRequest(method, params);
      if (!id) {
        reject(new Error("WebSocket not connected"));
        return;
      }
      this.responseWaiters.set(id, { resolve, reject });
      setTimeout(() => {
        if (this.responseWaiters.has(id)) {
          this.responseWaiters.delete(id);
          reject(new Error("Request timeout"));
        }
      }, 5000);
    });
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
    this.notifySessionChange();
  }

  /* ── Session history (localStorage) ── */
  private historyKey(sessionId: string): string {
    return `manta_history_${sessionId}`;
  }

  saveMessage(msg: ChatMessage): void {
    const key = this.historyKey(this.sessionId);
    const history: ChatMessage[] = JSON.parse(localStorage.getItem(key) || "[]");
    history.push({
      ...msg,
      timestamp: msg.timestamp ?? Date.now(),
    });
    // Keep last 200 messages
    if (history.length > 200) history.splice(0, history.length - 200);
    localStorage.setItem(key, JSON.stringify(history));
  }

  getHistory(sessionId: string): ChatMessage[] {
    try {
      return JSON.parse(localStorage.getItem(this.historyKey(sessionId)) || "[]");
    } catch {
      return [];
    }
  }

  async loadHistory(sessionId: string): Promise<ChatMessage[]> {
    try {
      // Wait for WebSocket to connect before requesting history
      // so we get the full backend data (reasoning + tool_calls)
      // instead of falling back to the text-only localStorage copy.
      await this.waitForConnected(8000);
      const res = await this.sendRequestAndWait("chat.history", {
        session_id: sessionId,
        limit: 200,
      }) as { messages?: Array<{
        id: string;
        role: string;
        content: string;
        reasoning_content?: string;
        tool_calls?: Array<{ id: string; call_type: string; function: { name: string; arguments: string }; result?: string }>;
        timestamp: number;
      }> } | undefined;
      const msgs = res?.messages || [];
      return msgs.map((m) => {
        const parts: ChatMessagePart[] = [];
        // Build parts: reasoning → tool calls → text
        if (m.reasoning_content) {
          parts.push({ type: "reasoning", text: m.reasoning_content });
        }
        if (m.tool_calls && m.tool_calls.length > 0) {
          for (const tc of m.tool_calls) {
            let args: Record<string, unknown> = {};
            try {
              args = JSON.parse(tc.function.arguments);
            } catch {
              args = { raw: tc.function.arguments };
            }
            parts.push({
              type: "tool-call",
              toolName: tc.function.name,
              args,
              result: tc.result,
            });
          }
        }
        if (m.content) {
          parts.push({ type: "text", text: m.content });
        }
        return {
          id: m.id,
          role: m.role as "user" | "assistant",
          content: m.content,
          parts: parts.length > 0 ? parts : undefined,
          durationMs: (m as any).duration_ms,
          toolCount: (m as any).tool_count,
        };
      });
    } catch {
      // Fallback to localStorage (already stores full ChatMessage with parts)
      return this.getHistory(sessionId);
    }
  }

  clearHistory(sessionId: string): void {
    localStorage.removeItem(this.historyKey(sessionId));
  }

  /* ── Config ── */
  async getConfig(): Promise<Record<string, unknown>> {
    const res = await this.sendRequestAndWait("config.get", {}) as Record<string, unknown> | undefined;
    return res || {};
  }

  async setConfig(path: string, value: unknown): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path, value });
      return true;
    } catch {
      return false;
    }
  }

  async listModels(): Promise<{ models: Array<{ id: string; name: string; provider: string }>; default_model: string }> {
    const res = await this.sendRequestAndWait("models.list", {}) as { models: Array<{ id: string; name: string; provider: string }>; default_model: string } | undefined;
    return res || { models: [], default_model: "" };
  }

  async listAgents(): Promise<{ agents: string[] }> {
    const res = await this.sendRequestAndWait("agents.list", {}) as { agents: string[] } | undefined;
    return res || { agents: [] };
  }

  async listAgentRegistry(): Promise<{ agents: Array<{ id: string; display_name: string; is_valid: boolean; has_heartbeat: boolean }>; count: number }> {
    const res = await this.sendRequestAndWait("agents.registry", {}) as { agents: Array<{ id: string; display_name: string; is_valid: boolean; has_heartbeat: boolean }>; count: number } | undefined;
    return res || { agents: [], count: 0 };
  }

  async getAgent(agentId: string): Promise<{
    agent_id: string;
    busy: boolean;
    status: string;
    config: Record<string, unknown> | null;
    personality: Record<string, unknown> | null;
  } | null> {
    const res = await this.sendRequestAndWait("agents.get", { agent_id: agentId }) as {
      agent_id: string;
      busy: boolean;
      status: string;
      config: Record<string, unknown> | null;
      personality: Record<string, unknown> | null;
    } | undefined;
    return res || null;
  }

  async listCrons(): Promise<{ jobs: Array<Record<string, unknown>>; count: number }> {
    const res = await this.sendRequestAndWait("cron.list", {}) as { jobs: Array<Record<string, unknown>>; count: number } | undefined;
    return res || { jobs: [], count: 0 };
  }

  async listSkills(): Promise<{ skills: Array<Record<string, unknown>>; count: number }> {
    const res = await this.sendRequestAndWait("skills.list", {}) as { skills: Array<Record<string, unknown>>; count: number } | undefined;
    return res || { skills: [], count: 0 };
  }

  async installSkill(name: string, zipBase64: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("skills.install", { name, zip_base64: zipBase64 });
      return true;
    } catch {
      return false;
    }
  }

  /* ── Model operations ── */
  async addModel(payload: { name: string; provider: string; model: string; api_key?: string }): Promise<boolean> {
    try {
      await this.sendRequestAndWait("models.add", payload);
      return true;
    } catch {
      return false;
    }
  }

  async removeModel(name: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("models.remove", { name });
      return true;
    } catch {
      return false;
    }
  }

  async setDefaultModel(name: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("models.set_default", { name });
      return true;
    } catch {
      return false;
    }
  }

  /* ── Log streaming ── */
  subscribeLogs(): void {
    this.sendRequest("logs.subscribe", {});
  }

  unsubscribeLogs(): void {
    this.sendRequest("logs.unsubscribe", {});
  }

  /* ── Channel operations ── */
  async addChannel(payload: { name: string; channel_type: string; enabled?: boolean; agent_id?: string; credentials?: Record<string, string> }): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path: "channels.add", value: payload });
      return true;
    } catch {
      return false;
    }
  }

  async updateChannel(payload: { name: string; enabled?: boolean; agent_id?: string; credentials?: Record<string, string> }): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path: "channels.update", value: payload });
      return true;
    } catch {
      return false;
    }
  }

  async removeChannel(name: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path: "channels.remove", value: name });
      return true;
    } catch {
      return false;
    }
  }

  async setChannelEnabled(name: string, enabled: boolean): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path: "channels.set_enabled", value: { name, enabled } });
      return true;
    } catch {
      return false;
    }
  }

  /* ── In-memory message state for UI ── */
  getServerInfo(): { version?: string; conn_id?: string; features?: string[]; scopes_granted?: string[] } {
    return this.serverInfo;
  }

  getGatewayUrl(): string {
    return this.gatewayUrl;
  }

  getMessages(): ChatMessage[] {
    return this.messages;
  }

  setMessages(msgs: ChatMessage[]): void {
    this.messages = msgs;
    this.messagesListeners.forEach((cb) => cb(msgs));
  }

  onMessagesChange(callback: MessagesCallback): () => void {
    this.messagesListeners.add(callback);
    callback(this.messages);
    return () => this.messagesListeners.delete(callback);
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

    const startTime = Date.now();

    const userMsg: ChatMessage = {
      id: `u_${Date.now()}`,
      role: "user",
      content: text,
    };
    // Save user message to history
    this.saveMessage(userMsg);
    this.messages = [...this.messages, userMsg];
    this.messagesListeners.forEach((cb) => cb(this.messages));

    // ── Slash command interception ──
    const parsed = parseCommand(text);
    if (parsed) {
      const cmd = findCommand(parsed.command);
      if (cmd) {
        // Local commands: handled client-side without RPC
        if (LOCAL_COMMANDS.has(parsed.command)) {
          if (parsed.command === "new") {
            this.createSession();
            this.setMessages([]);
            const cmdDuration = Date.now() - startTime;
            const assistantMsg: ChatMessage = {
              id: `a_${Date.now()}`,
              role: "assistant",
              content: "New session started.",
              durationMs: cmdDuration,
              toolCount: 0,
            };
            this.saveMessage(assistantMsg);
            this.messages = [...this.messages, assistantMsg];
            this.messagesListeners.forEach((cb) => cb(this.messages));
            yield {
              content: [makeTextPart("New session started.")],
              status: { type: "complete", reason: "stop" },
            };
            return;
          }
          if (parsed.command === "clear") {
            this.clearHistory(this.sessionId);
            this.setMessages([]);
            const cmdDuration = Date.now() - startTime;
            const assistantMsg: ChatMessage = {
              id: `a_${Date.now()}`,
              role: "assistant",
              content: "History cleared.",
              durationMs: cmdDuration,
              toolCount: 0,
            };
            this.saveMessage(assistantMsg);
            this.messages = [...this.messages, assistantMsg];
            this.messagesListeners.forEach((cb) => cb(this.messages));
            yield {
              content: [makeTextPart("History cleared.")],
              status: { type: "complete", reason: "stop" },
            };
            return;
          }
        }

        // Remote commands: send via RPC and yield response as assistant message
        try {
          await this.waitForConnected(5000);
          const result = (await this.sendRequestAndWait("commands.execute", {
            command: parsed.command,
            args: parsed.args,
            session_id: this.sessionId,
          })) as { text?: string } | undefined;

          const responseText = result?.text ?? "Command executed.";
          const cmdDuration = Date.now() - startTime;
          const assistantMsg: ChatMessage = {
            id: `a_${Date.now()}`,
            role: "assistant",
            content: responseText,
            durationMs: cmdDuration,
            toolCount: 0,
          };
          this.saveMessage(assistantMsg);
          this.messages = [...this.messages, assistantMsg];
          this.messagesListeners.forEach((cb) => cb(this.messages));
          yield {
            content: [makeTextPart(responseText)],
            status: { type: "complete", reason: "stop" },
          };
          return;
        } catch (err) {
          const errorText = `Command error: ${err instanceof Error ? err.message : String(err)}`;
          const cmdDuration = Date.now() - startTime;
          const assistantMsg: ChatMessage = {
            id: `a_${Date.now()}`,
            role: "assistant",
            content: errorText,
            durationMs: cmdDuration,
            toolCount: 0,
          };
          this.saveMessage(assistantMsg);
          this.messages = [...this.messages, assistantMsg];
          this.messagesListeners.forEach((cb) => cb(this.messages));
          yield {
            content: [makeTextPart(errorText)],
            status: { type: "complete", reason: "stop" },
          };
          return;
        }
      }

      // Unrecognized command: show error
      const errorText = `Unknown command: /${parsed.command}`;
      const cmdDuration = Date.now() - startTime;
      const assistantMsg: ChatMessage = {
        id: `a_${Date.now()}`,
        role: "assistant",
        content: errorText,
        durationMs: cmdDuration,
        toolCount: 0,
      };
      this.saveMessage(assistantMsg);
      this.messages = [...this.messages, assistantMsg];
      this.messagesListeners.forEach((cb) => cb(this.messages));
      yield {
        content: [makeTextPart(errorText)],
        status: { type: "complete", reason: "stop" },
      };
      return;
    }

    this.isRunningFlag = true;
    this.runListeners.forEach((cb) => cb(true));
    this.currentAbortController = new AbortController();

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
    let aiMsgId = `a_${Date.now()}`;
    let hasShownThinking = false;

    // Add empty AI message for streaming updates
    const aiMsg: ChatMessage = {
      id: aiMsgId,
      role: "assistant",
      content: "",
      parts: [],
    };
    this.messages = [...this.messages, aiMsg];
    this.messagesListeners.forEach((cb) => cb(this.messages));

    // Show "Thinking..." placeholder immediately while waiting for first event
    aiMsg.parts = [{ type: "reasoning", text: "" }];
    aiMsg.liveStatus = { status: "thinking" };
    hasShownThinking = true;
    this.messagesListeners.forEach((cb) => cb(this.messages));
    yield { content: [makeReasoningPart("")] };

    try {
      while (true) {
        const evt = await this.nextEvent(this.currentAbortController?.signal ?? abortSignal);
        if (!evt) {
          aborted = true;
          break;
        }

        switch (evt.event) {
          case "chat.delta": {
            const delta = (evt.payload?.delta as string) || (evt.payload?.content as string) || "";
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
            // Update in-memory AI message
            aiMsg.content = currentText;
            aiMsg.parts = newParts.map(toChatPart);
            this.messagesListeners.forEach((cb) => cb(this.messages));
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
            aiMsg.content = currentText;
            aiMsg.parts = newParts.map(toChatPart);
            // Only show thinking status on the first occurrence
            if (!hasShownThinking) {
              aiMsg.liveStatus = { status: "thinking" };
              hasShownThinking = true;
            }
            this.messagesListeners.forEach((cb) => cb(this.messages));
            yield { content: [...parts] };
            break;
          }
          case "tool.calling": {
            const toolName = (evt.payload?.tool_name as string) || "tool";
            const rawArgs = evt.payload?.arguments;
            let toolArgs: Record<string, unknown> = {};
            if (typeof rawArgs === "string") {
              try {
                toolArgs = JSON.parse(rawArgs) as Record<string, unknown>;
              } catch {
                toolArgs = { raw: rawArgs };
              }
            } else if (rawArgs && typeof rawArgs === "object") {
              toolArgs = rawArgs as Record<string, unknown>;
            }
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
            aiMsg.content = currentText;
            aiMsg.parts = newParts.map(toChatPart);
            aiMsg.liveStatus = { status: "tool_calling", toolName };
            this.messagesListeners.forEach((cb) => cb(this.messages));
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
            aiMsg.content = currentText;
            aiMsg.parts = newParts.map(toChatPart);
            this.messagesListeners.forEach((cb) => cb(this.messages));
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
            const durationMs = Date.now() - startTime;
            const toolCount = toolCalls.size;
            // Save final AI message to history with full parts and metadata
            this.saveMessage({ ...aiMsg, durationMs, toolCount });
            aiMsg.content = currentText;
            aiMsg.parts = finalParts.map(toChatPart);
            aiMsg.durationMs = durationMs;
            aiMsg.toolCount = toolCount;
            aiMsg.liveStatus = undefined;
            this.messagesListeners.forEach((cb) => cb(this.messages));
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
      this.isRunningFlag = false;
      this.runListeners.forEach((cb) => cb(false));
      this.currentAbortController = null;
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
