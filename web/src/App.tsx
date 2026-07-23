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
        const { messages: history, hasMore } = await transport.loadHistory(
          transport.getSessionId()
        );
        if (cancelled) return;
        transport.setMessages(history);
        useChatStore.getState().setMessages(history);
        useChatStore.getState().setHasMoreHistory(hasMore);
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
  const [sessions, setSessions] = useState<
    Array<{
      id: string;
      label?: string;
      agent_id?: string;
      pinned?: boolean;
      last_activity?: number;
    }>
  >([]);
  const [agents, setAgents] = useState<
    Array<{
      id: string;
      display_name: string;
      emoji: string;
      is_valid: boolean;
      has_heartbeat: boolean;
    }>
  >([]);
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

  // Load agents
  const refreshAgents = useCallback(async () => {
    const list = await transport.listAgentRegistry();
    // Exclude the default agent and any invalid entries.
    const filtered = list.filter(
      (a) => a.is_valid && a.id !== "default"
    );
    setAgents(filtered);
  }, [transport]);

  // Network status
  useEffect(() => {
    return transport.onStatusChange((status) => {
      setNetworkStatus(status);
      if (status === "connected") {
        refreshSessions();
        refreshAgents();
      }
    });
  }, [transport, refreshSessions, refreshAgents]);

  useEffect(() => {
    refreshSessions();
    refreshAgents();
    const interval = setInterval(() => {
      refreshSessions();
      refreshAgents();
    }, 8000);
    return () => clearInterval(interval);
  }, [refreshSessions, refreshAgents]);

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
      if (evt.event === "session.pinned") {
        const p = evt.payload as Record<string, unknown> | undefined;
        if (!p) return;
        const pinnedSessionId = p.session_id as string;
        const pinned = !!p.pinned;
        setSessions((prev) =>
          prev.map((s) =>
            s.id === pinnedSessionId ? { ...s, pinned } : s
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

  const handleCreateSessionWithAgent = useCallback(
    async (agentId: string) => {
      setSettingsOpen(false);

      // If a session already exists for this agent, switch to it instead
      // of creating another one.
      const existing = sessions.find((s) => s.agent_id === agentId);
      if (existing) {
        transport.switchSession(existing.id);
        const { messages: history, hasMore } = await transport.loadHistory(existing.id);
        transport.setMessages(history);
        useChatStore.getState().setHasMoreHistory(hasMore);
        setSessionKey((k) => k + 1);
        await refreshSessions();
        return;
      }

      transport.createSession(agentId);
      transport.setMessages([]);
      setSessionKey((k) => k + 1);
      await refreshSessions();
    },
    [transport, refreshSessions, sessions]
  );

  const handleSwitchSession = useCallback(
    async (id: string) => {
      setSettingsOpen(false);
      transport.switchSession(id);
      // Load history for the new session from backend
      const { messages: history, hasMore } = await transport.loadHistory(id);
      transport.setMessages(history);
      useChatStore.getState().setHasMoreHistory(hasMore);
      setSessionKey((k) => k + 1);
    },
    [transport]
  );

  const handleRenameSession = useCallback(
    async (id: string, name: string) => {
      await transport.renameSession(id, name);
      refreshSessions();
    },
    [transport, refreshSessions]
  );

  const handleDeleteSession = useCallback(
    async (id: string) => {
      await transport.deleteSession(id);
      refreshSessions();
      setSessionKey((k) => k + 1);
    },
    [transport, refreshSessions]
  );

  const handlePinSession = useCallback(
    async (id: string, pinned: boolean) => {
      await transport.setSessionPinned(id, pinned);
      refreshSessions();
    },
    [transport, refreshSessions]
  );

  // Build session items enriched with agent info for sidebar badges.
  const sessionItems = useMemo(() => {
    return sessions.map((s) => ({
      id: s.id,
      label: s.label,
      pinned: s.pinned,
      last_activity: s.last_activity,
      agent: agents.find((a) => a.id === s.agent_id),
    }));
  }, [sessions, agents]);

  return (
    <div className="h-screen flex bg-page text-primary">
      <Sidebar
        collapsed={sidebarCollapsed}
        onToggle={() => setSidebarCollapsed((c) => !c)}
        sessions={sessionItems}
        currentSessionId={transport.getSessionId()}
        onSwitchSession={handleSwitchSession}
        onNewSession={handleNewSession}
        agents={agents}
        onCreateSessionWithAgent={handleCreateSessionWithAgent}
        networkStatus={networkStatus}
        onOpenSettings={() => setSettingsOpen((s) => !s)}
        onRenameSession={handleRenameSession}
        onDeleteSession={handleDeleteSession}
        onPinSession={handlePinSession}
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
