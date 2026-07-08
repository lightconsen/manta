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
import { useGoalStore } from "@/stores/goalStore";
import { GoalPanel } from "@/components/chat/GoalPanel";

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
      if (evt.event === "goal.progress") {
        const p = evt.payload as Record<string, unknown> | undefined;
        if (!p) return;
        const goalId = (p.goal_id as string) || "";
        const goalEvent = p.event as Record<string, unknown> | undefined;
        if (!goalEvent) return;
        const subEvent = (goalEvent.event as string) || "";

        // Update goal store for the dedicated GoalPanel.
        const updateGoal = useGoalStore.getState().updateGoal;
        if (subEvent === "goal.started") {
          const desc = (goalEvent.description as string) || "";
          const conditions = (goalEvent.conditions as string[]) || [];
          const maxRounds = (goalEvent.max_rounds as number) || 5;
          useGoalStore.setState((s) => ({
            goals: {
              ...s.goals,
              [goalId]: {
                id: goalId,
                description: desc,
                conditions,
                maxRounds,
                round: 0,
                passed: 0,
                total: conditions.length,
                status: "running",
              },
            },
          }));
        } else if (subEvent === "goal.check") {
          updateGoal(goalId, {
            round: (goalEvent.round as number) || 0,
            passed: (goalEvent.passed as number) || 0,
            total: (goalEvent.total as number) || 0,
          });
        } else if (subEvent === "goal.done") {
          updateGoal(goalId, {
            status: "done",
            summary: (goalEvent.summary as string) || "",
          });
        } else if (subEvent === "goal.aborted") {
          updateGoal(goalId, {
            status: "aborted",
            reason: (goalEvent.reason as string) || "",
            round: (goalEvent.round as number) || 0,
          });
        }

        // Build chat message (existing behavior).
        let text = "";
        if (subEvent === "goal.started") {
          const desc = (goalEvent.description as string) || "";
          const conditions = (goalEvent.conditions as string[]) || [];
          const maxRounds = (goalEvent.max_rounds as number) || 5;
          const condList = conditions.map((c: string) => `  - ${c}`).join("\n");
          text = `🎯 **Goal Started**: ${desc}\n\n**Conditions** (must all pass):\n${condList}\n\n_Max rounds: ${maxRounds}_`;
        } else if (subEvent === "goal.check") {
          const round = (goalEvent.round as number) || 0;
          const passed = (goalEvent.passed as number) || 0;
          const total = (goalEvent.total as number) || 0;
          text = `🔍 **Goal Check — Round ${round}**: ${passed}/${total} conditions passed`;
        } else if (subEvent === "goal.retry") {
          const round = (goalEvent.round as number) || 0;
          const feedback = (goalEvent.feedback as string) || "";
          text = `🔄 **Goal Retry — Round ${round}**\n\n${feedback}`;
        } else if (subEvent === "goal.done") {
          const summary = (goalEvent.summary as string) || "";
          text = `✅ **Goal Complete**\n\n${summary}`;
        } else if (subEvent === "goal.aborted") {
          const reason = (goalEvent.reason as string) || "";
          text = `⛔ **Goal Aborted**: ${reason}`;
        }
        if (text) {
          const msg: ChatMessage = {
            id: `goal_${Date.now()}`,
            role: "assistant",
            content: text,
            parts: [{ type: "text", text }],
            timestamp: Date.now(),
          };
          transport.saveMessage(msg);
          const updated = [...transport.getMessages(), msg];
          transport.setMessages(updated);
        }
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
          <>
            <ChatAppInner key={sessionKey} transport={transport} />
            <GoalPanel />
          </>
        )}
      </main>
    </div>
  );
}

export default ChatApp;
