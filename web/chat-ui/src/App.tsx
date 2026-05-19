import { useMemo, useState, useEffect, useCallback } from "react";
import {
  AssistantRuntimeProvider,
  ThreadPrimitive,
  ComposerPrimitive,
  useLocalRuntime,
} from "@assistant-ui/react";
import {
  MantaWebSocketTransport,
  type NetworkStatus,
  type ChatMessage,
} from "./MantaWebSocketTransport";
import { MarkdownMessage } from "./components/MarkdownMessage";
import { ReasoningPart } from "./components/ReasoningPart";
import { ToolCallPart } from "./components/ToolCallPart";

/* ── Icons ── */
function LogoIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 100 80"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
    >
      {/* Manta ray silhouette */}
      <path
        d="M50 8
           C50 8, 38 0, 28 8
           C18 16, 8 24, 2 36
           C-2 44, 2 52, 10 48
           C18 44, 22 40, 26 36
           C30 32, 34 28, 38 30
           C42 32, 44 38, 44 46
           C44 54, 42 64, 40 72
           C38 76, 42 78, 44 74
           C46 66, 48 56, 50 50
           C52 56, 54 66, 56 74
           C58 78, 62 76, 60 72
           C58 64, 56 54, 56 46
           C56 38, 58 32, 62 30
           C66 28, 70 32, 74 36
           C78 40, 82 44, 90 48
           C98 52, 102 44, 98 36
           C92 24, 82 16, 72 8
           C62 0, 50 8, 50 8Z"
        fill="currentColor"
      />
      {/* Eyes */}
      <circle cx="38" cy="18" r="2" fill="white" />
      <circle cx="62" cy="18" r="2" fill="white" />
    </svg>
  );
}

function ChevronLeftIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="15 18 9 12 15 6" />
    </svg>
  );
}

function ChevronRightIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="9 18 15 12 9 6" />
    </svg>
  );
}

function PlusIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  );
}

function SunIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="5" />
      <line x1="12" y1="1" x2="12" y2="3" />
      <line x1="12" y1="21" x2="12" y2="23" />
      <line x1="4.22" y1="4.22" x2="5.64" y2="5.64" />
      <line x1="18.36" y1="18.36" x2="19.78" y2="19.78" />
      <line x1="1" y1="12" x2="3" y2="12" />
      <line x1="21" y1="12" x2="23" y2="12" />
      <line x1="4.22" y1="19.78" x2="5.64" y2="18.36" />
      <line x1="18.36" y1="5.64" x2="19.78" y2="4.22" />
    </svg>
  );
}

function MoonIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  );
}

function SettingsIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function StatusDot({ status }: { status: NetworkStatus }) {
  const color =
    status === "connected"
      ? "bg-green-500"
      : status === "connecting"
      ? "bg-yellow-500 animate-pulse"
      : "bg-red-500";
  return <span className={`w-2 h-2 rounded-full ${color}`} />;
}

/* ── Sidebar ── */
function Sidebar({
  collapsed,
  onToggle,
  sessions,
  currentSessionId,
  onSwitchSession,
  onNewSession,
  networkStatus,
  theme,
  onToggleTheme,
}: {
  collapsed: boolean;
  onToggle: () => void;
  sessions: Array<{ id: string; label?: string }>;
  currentSessionId: string;
  onSwitchSession: (id: string) => void;
  onNewSession: () => void;
  networkStatus: NetworkStatus;
  theme: "light" | "dark";
  onToggleTheme: () => void;
}) {
  return (
    <aside
      className={`shrink-0 flex flex-col bg-gray-50 dark:bg-neutral-950 border-r border-gray-200 dark:border-neutral-800 transition-all duration-300 ${
        collapsed ? "w-16" : "w-64"
      }`}
    >
      {/* Top: Logo + Name + Collapse */}
      <div className="h-14 flex items-center justify-between px-3 border-b border-gray-200 dark:border-neutral-800 shrink-0">
        <div className="flex items-center gap-2 overflow-hidden">
          <LogoIcon className="w-6 h-6 text-emerald-500 shrink-0" />
          {!collapsed && (
            <span className="text-sm font-semibold text-gray-900 dark:text-gray-100 whitespace-nowrap">
              Manta
            </span>
          )}
        </div>
        <button
          onClick={onToggle}
          className="p-1 rounded-md hover:bg-gray-200 dark:hover:bg-neutral-800 text-gray-500 dark:text-neutral-400 transition shrink-0"
          title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
        >
          {collapsed ? (
            <ChevronRightIcon className="w-4 h-4" />
          ) : (
            <ChevronLeftIcon className="w-4 h-4" />
          )}
        </button>
      </div>

      {/* Middle: Session List */}
      <div className="flex-1 overflow-y-auto py-2">
        {!collapsed && sessions.length > 0 && (
          <div className="px-3 pb-1">
            <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 font-medium">
              Sessions
            </span>
          </div>
        )}
        {sessions.map((s) => (
          <button
            key={s.id}
            onClick={() => onSwitchSession(s.id)}
            className={`w-full text-left px-3 py-2 mx-1 rounded-lg text-sm transition flex items-center gap-2 ${
              s.id === currentSessionId
                ? "bg-blue-50 dark:bg-blue-900/20 text-blue-700 dark:text-blue-300"
                : "text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-neutral-800"
            } ${collapsed ? "justify-center" : ""}`}
            title={s.id}
          >
            <span className="w-5 h-5 rounded-full bg-gradient-to-br from-blue-400 to-indigo-500 shrink-0 flex items-center justify-center text-[10px] text-white font-bold">
              {s.label?.charAt(0).toUpperCase() || "S"}
            </span>
            {!collapsed && (
              <span className="truncate">{s.label || s.id}</span>
            )}
          </button>
        ))}
        <button
          onClick={onNewSession}
          className={`w-full text-left px-3 py-2 mx-1 mt-1 rounded-lg text-sm transition flex items-center gap-2 text-gray-500 dark:text-neutral-400 hover:bg-gray-100 dark:hover:bg-neutral-800 ${
            collapsed ? "justify-center" : ""
          }`}
          title="New session"
        >
          <PlusIcon className="w-4 h-4 shrink-0" />
          {!collapsed && <span>New Session</span>}
        </button>
      </div>

      {/* Bottom: Network + Theme + Settings */}
      <div className="border-t border-gray-200 dark:border-neutral-800 p-3 shrink-0">
        <div className={`flex items-center ${collapsed ? "justify-center" : "justify-between"}`}>
          {!collapsed && (
            <div className="flex items-center gap-2 text-xs text-gray-500 dark:text-neutral-400">
              <StatusDot status={networkStatus} />
              <span className="capitalize">{networkStatus}</span>
            </div>
          )}
          {collapsed && <StatusDot status={networkStatus} />}
          <div className="flex items-center gap-1">
            <button
              onClick={onToggleTheme}
              className="p-1.5 rounded-md hover:bg-gray-200 dark:hover:bg-neutral-800 text-gray-500 dark:text-neutral-400 transition"
              title="Toggle theme"
            >
              {theme === "dark" ? (
                <SunIcon className="w-4 h-4" />
              ) : (
                <MoonIcon className="w-4 h-4" />
              )}
            </button>
            <button
              className="p-1.5 rounded-md hover:bg-gray-200 dark:hover:bg-neutral-800 text-gray-500 dark:text-neutral-400 transition"
              title="Settings"
            >
              <SettingsIcon className="w-4 h-4" />
            </button>
          </div>
        </div>
      </div>
    </aside>
  );
}

/* ── Avatar ── */
function Avatar({ role }: { role: string }) {
  if (role === "user") {
    return (
      <div className="w-7 h-7 rounded-full bg-gradient-to-br from-blue-500 to-indigo-600 flex items-center justify-center text-white text-[11px] font-bold shrink-0">
        U
      </div>
    );
  }
  return (
    <div className="w-7 h-7 rounded-full bg-gradient-to-br from-emerald-500 to-teal-600 flex items-center justify-center text-white shrink-0">
      <LogoIcon className="w-3.5 h-3.5" />
    </div>
  );
}

/* ── Message Bubble ── */
function MessageBubble({ message }: { message: ChatMessage }) {
  const isUser = message.role === "user";

  if (isUser) {
    return (
      <div className="py-4 px-4 sm:px-6 bg-white dark:bg-neutral-900">
        <div className="max-w-3xl mx-auto flex gap-3 flex-row-reverse">
          <Avatar role="user" />
          <div className="flex-1 min-w-0 text-right">
            <div className="text-[11px] font-medium text-gray-400 dark:text-neutral-500 mb-1 uppercase tracking-wide">
              You
            </div>
            <div className="inline-block text-left rounded-2xl px-4 py-2.5 bg-blue-600 text-white rounded-br-md">
              <p className="text-sm leading-relaxed whitespace-pre-wrap">
                {message.content}
              </p>
            </div>
          </div>
        </div>
      </div>
    );
  }

  // Assistant message – render parts (reasoning, tool calls, text)
  const hasParts = message.parts && message.parts.length > 0;

  return (
    <div className="py-4 px-4 sm:px-6 bg-gray-50/60 dark:bg-neutral-800/30">
      <div className="max-w-3xl mx-auto flex gap-3 flex-row">
        <Avatar role="assistant" />
        <div className="flex-1 min-w-0">
          <div className="text-[11px] font-medium text-gray-400 dark:text-neutral-500 mb-1 uppercase tracking-wide">
            Manta
          </div>
          {hasParts ? (
            <div className="space-y-1">
              {message.parts!.map((part, i) => {
                if (part.type === "reasoning") {
                  return <ReasoningPart key={i} text={part.text || ""} />;
                }
                if (part.type === "tool-call") {
                  return (
                    <ToolCallPart
                      key={i}
                      toolName={part.toolName || "tool"}
                      args={part.args || {}}
                      result={part.result}
                    />
                  );
                }
                if (part.type === "text") {
                  return (
                    <div className="rounded-2xl px-4 py-2.5 bg-white dark:bg-neutral-800 text-gray-800 dark:text-gray-200 rounded-bl-md shadow-sm border border-gray-100 dark:border-neutral-700">
                      <MarkdownMessage text={part.text || ""} />
                    </div>
                  );
                }
                return null;
              })}
            </div>
          ) : (
            <div className="rounded-2xl px-4 py-2.5 bg-white dark:bg-neutral-800 text-gray-800 dark:text-gray-200 rounded-bl-md shadow-sm border border-gray-100 dark:border-neutral-700">
              <MarkdownMessage text={message.content} />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/* ── Chat Content ── */
function ChatContent({ messages }: { messages: ChatMessage[] }) {
  return (
    <ThreadPrimitive.Root className="flex-1 flex flex-col overflow-hidden">
      <ThreadPrimitive.Viewport className="flex-1 overflow-y-auto">
        {messages.length === 0 && (
          <div className="flex items-center justify-center h-full">
            <div className="text-center">
              <div className="w-12 h-12 rounded-2xl bg-gradient-to-br from-emerald-500 to-teal-600 flex items-center justify-center text-white mx-auto mb-4 shadow-lg shadow-emerald-500/20">
                <LogoIcon className="w-6 h-6" />
              </div>
              <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100 mb-1">
                Manta
              </h2>
              <p className="text-gray-400 dark:text-neutral-500 text-sm">
                Start a conversation
              </p>
            </div>
          </div>
        )}
        {messages.map((msg) => (
          <MessageBubble key={msg.id} message={msg} />
        ))}
      </ThreadPrimitive.Viewport>

      <div className="bg-white dark:bg-neutral-900 px-4 py-3 shrink-0">
        <ComposerPrimitive.Root className="flex items-end gap-2 max-w-3xl mx-auto">
          {/* Attachment buttons */}
          <div className="flex items-center gap-1 shrink-0 pb-1">
            <button
              type="button"
              title="Voice input"
              className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-emerald-500 dark:hover:text-emerald-400 hover:bg-gray-100 dark:hover:bg-neutral-800 transition"
              onClick={() => alert('Voice input coming soon')}
            >
              <svg className="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
                <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
                <line x1="12" y1="19" x2="12" y2="23" />
                <line x1="8" y1="23" x2="16" y2="23" />
              </svg>
            </button>
            <button
              type="button"
              title="Upload image"
              className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-emerald-500 dark:hover:text-emerald-400 hover:bg-gray-100 dark:hover:bg-neutral-800 transition"
              onClick={() => alert('Image upload coming soon')}
            >
              <svg className="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                <circle cx="8.5" cy="8.5" r="1.5" />
                <polyline points="21 15 16 10 5 21" />
              </svg>
            </button>
            <button
              type="button"
              title="Upload file"
              className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-emerald-500 dark:hover:text-emerald-400 hover:bg-gray-100 dark:hover:bg-neutral-800 transition"
              onClick={() => alert('File upload coming soon')}
            >
              <svg className="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
              </svg>
            </button>
          </div>

          <ComposerPrimitive.Input
            className="flex-1 resize-none rounded-xl border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 px-4 py-2.5 text-sm text-gray-900 dark:text-gray-100 placeholder-gray-400 dark:placeholder-neutral-500 focus:outline-none focus:ring-2 focus:ring-emerald-500/30 focus:border-emerald-500/50 transition min-h-[44px] max-h-[120px]"
            placeholder="Message Manta..."
          />
          <ComposerPrimitive.Send className="shrink-0 px-4 py-2.5 rounded-xl bg-gradient-to-r from-emerald-500 to-teal-600 hover:from-emerald-600 hover:to-teal-700 disabled:opacity-40 text-white text-sm font-medium transition shadow-sm">
            <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
              <line x1="22" y1="2" x2="11" y2="13" />
              <polygon points="22 2 15 22 11 13 2 9 22 2" />
            </svg>
          </ComposerPrimitive.Send>
        </ComposerPrimitive.Root>
      </div>
    </ThreadPrimitive.Root>
  );
}

/* ── App ── */
function ChatAppInner({ transport }: { transport: MantaWebSocketTransport }) {
  const runtime = useLocalRuntime(transport);
  const [messages, setMessages] = useState<ChatMessage[]>([]);

  useEffect(() => {
    let cancelled = false;
    transport.loadHistory(transport.getSessionId()).then((history) => {
      if (cancelled) return;
      const initialMessages: ChatMessage[] = history.map((h) => ({
        id: h.id,
        role: h.role,
        content: h.content,
      }));
      transport.setMessages(initialMessages);
      setMessages(initialMessages);
    });

    // Subscribe to message changes
    const unsub = transport.onMessagesChange((msgs) => {
      setMessages([...msgs]);
    });
    return () => {
      cancelled = true;
      unsub();
    };
  }, [transport]);

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ChatContent messages={messages} />
    </AssistantRuntimeProvider>
  );
}

function ChatApp() {
  const transport = useMemo(() => new MantaWebSocketTransport(), []);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    return localStorage.getItem("manta_sidebar_collapsed") === "true";
  });
  const [sessions, setSessions] = useState<Array<{ id: string; label?: string }>>([]);
  const [networkStatus, setNetworkStatus] = useState<NetworkStatus>("connecting");
  const [theme, setTheme] = useState<"light" | "dark">(() => {
    const stored = localStorage.getItem("manta_theme");
    if (stored === "light" || stored === "dark") return stored;
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  });
  const [sessionKey, setSessionKey] = useState(0);

  // Apply theme
  useEffect(() => {
    if (theme === "dark") {
      document.documentElement.classList.add("dark");
    } else {
      document.documentElement.classList.remove("dark");
    }
    localStorage.setItem("manta_theme", theme);
  }, [theme]);

  // Sidebar state persistence
  useEffect(() => {
    localStorage.setItem("manta_sidebar_collapsed", String(sidebarCollapsed));
  }, [sidebarCollapsed]);

  // Network status
  useEffect(() => {
    return transport.onStatusChange((status) => setNetworkStatus(status));
  }, [transport]);

  // Load sessions
  const refreshSessions = useCallback(async () => {
    const list = await transport.listSessions();
    setSessions(list);
  }, [transport]);

  useEffect(() => {
    refreshSessions();
    const interval = setInterval(refreshSessions, 8000);
    return () => clearInterval(interval);
  }, [refreshSessions]);

  // Listen for new sessions
  useEffect(() => {
    return transport.onEvent((evt) => {
      if (evt.event === "session.created") {
        refreshSessions();
      }
    });
  }, [transport, refreshSessions]);

  const handleNewSession = useCallback(() => {
    transport.createSession();
    transport.setMessages([]);
    setSessionKey((k) => k + 1);
    setTimeout(refreshSessions, 500);
  }, [transport, refreshSessions]);

  const handleSwitchSession = useCallback(
    async (id: string) => {
      transport.switchSession(id);
      // Load history for the new session from backend
      const history = await transport.loadHistory(id);
      const initialMessages: ChatMessage[] = history.map((h) => ({
        id: h.id,
        role: h.role,
        content: h.content,
      }));
      transport.setMessages(initialMessages);
      setSessionKey((k) => k + 1);
    },
    [transport]
  );

  return (
    <div className="h-screen flex bg-white dark:bg-neutral-900">
      <Sidebar
        collapsed={sidebarCollapsed}
        onToggle={() => setSidebarCollapsed((c) => !c)}
        sessions={sessions}
        currentSessionId={transport.getSessionId()}
        onSwitchSession={handleSwitchSession}
        onNewSession={handleNewSession}
        networkStatus={networkStatus}
        theme={theme}
        onToggleTheme={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}
      />
      <main className="flex-1 flex flex-col overflow-hidden">
        <ChatAppInner key={sessionKey} transport={transport} />
      </main>
    </div>
  );
}

export default ChatApp;
