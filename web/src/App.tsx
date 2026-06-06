import { useMemo, useState, useEffect, useCallback } from "react";
import {
  AssistantRuntimeProvider,
  useLocalRuntime,
} from "@assistant-ui/react";
import {
  SyscityWebSocketTransport,
  type NetworkStatus,
  type ChatMessage,
} from "@/SyscityWebSocketTransport";
import { useChatStore } from "@/stores/chatStore";
import { Sidebar } from "@/components/chat/Sidebar";
import { SettingsPanel } from "@/components/settings/SettingsPanel";
import { ChatContent } from "@/components/chat/ChatContent";

/* ── ChatAppInner ── */
function ChatAppInner({ transport }: { transport: SyscityWebSocketTransport }) {
  const runtime = useLocalRuntime(transport);

  useEffect(() => {
    let cancelled = false;
    const doLoad = async () => {
      try {
        const history = await transport.loadHistory(transport.getSessionId());
        if (cancelled) return;
        transport.setMessages(history);
        useChatStore.getState().setMessages(history);
      } catch {
        /* ignore — will retry when connected */
      }
    };
    doLoad();

    // Retry loading history when connection is established
    const unsubStatus = transport.onStatusChange((status) => {
      if (status === "connected" && transport.getMessages().length === 0) {
        doLoad();
      }
    });

    // Subscribe to message changes
    const unsub = transport.onMessagesChange((msgs) => {
      useChatStore.getState().setMessages([...msgs]);
    });
    return () => {
      cancelled = true;
      unsub();
      unsubStatus();
    };
  }, [transport]);

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ChatContent transport={transport} />
    </AssistantRuntimeProvider>
  );
}

/* ── ChatApp ── */
function ChatApp() {
  const transport = useMemo(() => new SyscityWebSocketTransport(), []);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    return localStorage.getItem("syscity_sidebar_collapsed") === "true";
  });
  const [sessions, setSessions] = useState<Array<{ id: string; label?: string }>>([]);
  const [networkStatus, setNetworkStatus] = useState<NetworkStatus>("connecting");
  const [sessionKey, setSessionKey] = useState(0);
  const [settingsOpen, setSettingsOpen] = useState(false);

  // Sidebar state persistence
  useEffect(() => {
    localStorage.setItem("syscity_sidebar_collapsed", String(sidebarCollapsed));
  }, [sidebarCollapsed]);

  // Load sessions
  const refreshSessions = useCallback(async () => {
    const list = await transport.listSessions();
    setSessions(list);
  }, [transport]);

  // Network status
  useEffect(() => {
    return transport.onStatusChange((status) => {
      setNetworkStatus(status);
      if (status === "connected") {
        refreshSessions();
      }
    });
  }, [transport, refreshSessions]);

  useEffect(() => {
    refreshSessions();
    const interval = setInterval(refreshSessions, 8000);
    return () => clearInterval(interval);
  }, [refreshSessions]);

  // Refresh session list immediately when transport creates/switches sessions
  useEffect(() => {
    return transport.onSessionChange(() => {
      refreshSessions();
    });
  }, [transport, refreshSessions]);

  // Listen for new sessions, renames, and cron results
  useEffect(() => {
    return transport.onEvent((evt) => {
      if (evt.event === "session.created") {
        refreshSessions();
      }
      if (evt.event === "session.renamed") {
        const p = evt.payload as Record<string, string> | undefined;
        if (!p) return;
        const renamedSessionId = p.session_id;
        const newName = p.name;
        setSessions((prev) =>
          prev.map((s) =>
            s.id === renamedSessionId
              ? { ...s, label: newName }
              : s
          )
        );
      }
      if (evt.event === "cron.completed") {
        const p = evt.payload as Record<string, string> | undefined;
        if (!p) return;
        const jobName = p.job_name || "cron job";
        const status = p.status || "ok";
        const output = p.output || "";
        const runAt = p.run_at || "";
        const icon = status === "ok" ? "✅" : "❌";
        const text = `${icon} **${jobName}**\n\n${output}\n\n_Executed at ${runAt}_`;
        const msg: ChatMessage = {
          id: `cron_${Date.now()}`,
          role: "assistant",
          content: text,
          parts: [{ type: "text", text }],
          timestamp: Date.now(),
        };
        transport.saveMessage(msg);
        const updated = [...transport.getMessages(), msg];
        transport.setMessages(updated);
      }
    });
  }, [transport, refreshSessions]);

  const handleNewSession = useCallback(() => {
    setSettingsOpen(false);
    // Don't create a new session if the current one already has no messages
    if (transport.getMessages().length === 0) {
      transport.setMessages([]);
      setSessionKey((k) => k + 1);
      return;
    }
    transport.createSession();
    transport.setMessages([]);
    setSessionKey((k) => k + 1);
    refreshSessions();
  }, [transport, refreshSessions]);

  const handleSwitchSession = useCallback(
    async (id: string) => {
      setSettingsOpen(false);
      transport.switchSession(id);
      // Load history for the new session from backend
      const history = await transport.loadHistory(id);
      transport.setMessages(history);
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
        onOpenSettings={() => setSettingsOpen((s) => !s)}
      />
      <main className="flex-1 flex flex-col overflow-hidden">
        {settingsOpen ? (
          <SettingsPanel transport={transport} onClose={() => setSettingsOpen(false)} />
        ) : (
          <ChatAppInner key={sessionKey} transport={transport} />
        )}
      </main>
    </div>
  );
}

export default ChatApp;
