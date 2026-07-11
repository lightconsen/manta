import {
  ChevronLeft,
  ChevronRight,
  Plus,
  Sun,
  Moon,
  Settings,
  Bot,
  HeartPulse,
  Pencil,
  Trash2,
  Pin,
  PinOff,
} from "lucide-react";
import { useThemeStore } from "@/stores/themeStore";
import { StatusDot } from "./StatusDot";
import type { NetworkStatus } from "@/SyscityWebSocketTransport";
import { useState, useRef, useCallback, useMemo, useEffect } from "react";

interface AgentItem {
  id: string;
  display_name: string;
  emoji: string;
  is_valid: boolean;
  has_heartbeat: boolean;
}

interface SessionItem {
  id: string;
  label?: string;
  pinned?: boolean;
  last_activity?: number;
  agent?: AgentItem;
}

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
  sessions: SessionItem[];
  currentSessionId: string;
  onSwitchSession: (id: string) => void;
  onNewSession: () => void;
  agents: AgentItem[];
  onCreateSessionWithAgent: (agentId: string) => void;
  networkStatus: NetworkStatus;
  onOpenSettings: () => void;
  onRenameSession?: (id: string, name: string) => void | Promise<void>;
  onDeleteSession?: (id: string) => void | Promise<void>;
  onPinSession?: (id: string, pinned: boolean) => void | Promise<void>;
}

export function Sidebar({
  collapsed,
  onToggle,
  sessions,
  currentSessionId,
  onSwitchSession,
  onNewSession,
  agents,
  onCreateSessionWithAgent,
  networkStatus,
  onOpenSettings,
  onRenameSession,
  onDeleteSession,
  onPinSession,
}: SidebarProps) {
  const { resolvedTheme, setTheme } = useThemeStore();

  const listContainerRef = useRef<HTMLDivElement>(null);
  const [sessionRatio, setSessionRatio] = useState<number>(() => {
    const saved = localStorage.getItem("syscity_sidebar_session_ratio");
    if (saved) {
      const v = parseFloat(saved);
      if (!isNaN(v)) return Math.max(0.2, Math.min(0.8, v));
    }
    return 0.6;
  });
  const [isDragging, setIsDragging] = useState(false);

  useEffect(() => {
    localStorage.setItem("syscity_sidebar_session_ratio", String(sessionRatio));
  }, [sessionRatio]);

  const handleMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (collapsed) return;
      e.preventDefault();
      setIsDragging(true);
      document.body.style.userSelect = "none";

      const container = listContainerRef.current;
      if (!container) return;
      const rect = container.getBoundingClientRect();

      const handleMouseMove = (moveEvent: MouseEvent) => {
        const y = moveEvent.clientY - rect.top;
        const ratio = Math.max(0.15, Math.min(0.85, y / rect.height));
        setSessionRatio(ratio);
      };

      const handleMouseUp = () => {
        setIsDragging(false);
        document.body.style.userSelect = "";
        window.removeEventListener("mousemove", handleMouseMove);
        window.removeEventListener("mouseup", handleMouseUp);
      };

      window.addEventListener("mousemove", handleMouseMove);
      window.addEventListener("mouseup", handleMouseUp);
    },
    [collapsed]
  );

  const groups = useMemo(() => {
    const now = new Date();
    const startOfToday = new Date(
      now.getFullYear(),
      now.getMonth(),
      now.getDate()
    ).getTime();

    const pinned = sessions
      .filter((s) => s.pinned)
      .sort((a, b) => (b.last_activity || 0) - (a.last_activity || 0));

    const buckets = new Map<string, SessionItem[]>();
    for (const s of sessions) {
      if (s.pinned) continue;
      const ts = s.last_activity || 0;
      const d = new Date(ts);
      const dayStart = new Date(
        d.getFullYear(),
        d.getMonth(),
        d.getDate()
      ).getTime();
      const diffDays = Math.floor(
        (startOfToday - dayStart) / (1000 * 60 * 60 * 24)
      );
      let key: string;
      if (diffDays <= 0) {
        key = "Today";
      } else if (diffDays === 1) {
        key = "Yesterday";
      } else if (diffDays <= 7) {
        key = "Last 7 days";
      } else if (diffDays <= 30) {
        key = "Last 30 days";
      } else {
        key = "Older";
      }
      if (!buckets.has(key)) {
        buckets.set(key, []);
      }
      buckets.get(key)!.push(s);
    }

    const order = ["Today", "Yesterday", "Last 7 days", "Last 30 days", "Older"];
    const result: { label: string; sessions: SessionItem[] }[] = [];
    if (pinned.length > 0) {
      result.push({ label: "Pinned", sessions: pinned });
    }
    for (const key of order) {
      const list = buckets.get(key);
      if (list && list.length > 0) {
        list.sort((a, b) => (b.last_activity || 0) - (a.last_activity || 0));
        result.push({ label: key, sessions: list });
      }
    }
    return result;
  }, [sessions]);

  return (
    <aside
      className={`shrink-0 flex flex-col bg-sidebar transition-all duration-300 overflow-x-hidden ${
        collapsed ? "w-16" : "w-64"
      }`}
    >
      {/* Top: Logo + Name + Collapse */}
      <div className="h-14 flex items-center justify-between px-3 shrink-0">
        <div className="flex items-center gap-2 overflow-hidden">
          <img
            src="/syscity.png"
            alt="Syscity"
            className="w-6 h-6 shrink-0"
            draggable={false}
          />
          {!collapsed && (
            <span className="text-sm font-semibold text-primary whitespace-nowrap">
              Syscity
            </span>
          )}
        </div>
        <button
          onClick={onToggle}
          className="p-1 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary transition shrink-0"
          title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
        >
          {collapsed ? (
            <ChevronRight className="w-4 h-4" />
          ) : (
            <ChevronLeft className="w-4 h-4" />
          )}
        </button>
      </div>

      <div
        ref={listContainerRef}
        className="flex-1 flex flex-col min-h-0 overflow-hidden"
      >
        {/* Sessions */}
        <div
          className="overflow-y-auto overflow-x-hidden px-1 py-2"
          style={{ flexBasis: `${sessionRatio * 100}%`, flexShrink: 0, flexGrow: 0 }}
          role="list"
        >
          <button
            onClick={onNewSession}
            className={`w-full text-left px-3 py-2 mb-2 rounded-lg text-sm transition flex items-center gap-2 text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04] ${
              collapsed ? "justify-center" : ""
            }`}
            title="New session"
            aria-label="New session"
          >
            <Plus className="w-4 h-4 shrink-0" />
            {!collapsed && <span>New Session</span>}
          </button>
          {!collapsed &&
            groups.map((group) => (
              <div key={group.label} className="mb-2">
                <div className="px-3 pb-1">
                  <span className="text-[10px] uppercase tracking-wider text-secondary font-medium">
                    {group.label}
                  </span>
                </div>
                {group.sessions.map((s) => (
                  <SessionRow
                    key={s.id}
                    session={s}
                    currentSessionId={currentSessionId}
                    collapsed={collapsed}
                    onSwitch={() => onSwitchSession(s.id)}
                    onRename={onRenameSession}
                    onDelete={onDeleteSession}
                    onPin={onPinSession}
                  />
                ))}
              </div>
            ))}
          {collapsed &&
            sessions.map((s) => (
              <SessionRow
                key={s.id}
                session={s}
                currentSessionId={currentSessionId}
                collapsed={collapsed}
                onSwitch={() => onSwitchSession(s.id)}
                onRename={onRenameSession}
                onDelete={onDeleteSession}
                onPin={onPinSession}
              />
            ))}
        </div>

        {/* Resize handle */}
        {!collapsed && (
          <div
            onMouseDown={handleMouseDown}
            className={`relative h-1 shrink-0 cursor-ns-resize bg-transparent hover:bg-primary/10 transition-colors ${
              isDragging ? "bg-primary/20" : ""
            }`}
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize sessions and agents panels"
            title="Drag to resize"
          >
            <div className="absolute left-1/2 -translate-x-1/2 top-1/2 -translate-y-1/2 w-8 h-0.5 rounded-full bg-subtle" />
          </div>
        )}

        {/* Agents */}
        <div
          className="flex flex-col min-h-0 border-t border-subtle"
          style={{
            flexBasis: `${(1 - sessionRatio) * 100}%`,
            flexShrink: 0,
            flexGrow: 0,
          }}
        >
          {!collapsed && (
            <div className="px-3 py-2 shrink-0">
              <span className="text-[10px] uppercase tracking-wider text-secondary font-medium">
                Agents
              </span>
            </div>
          )}
          <div className="flex-1 overflow-y-auto overflow-x-hidden px-1 py-1" role="list">
            {agents.length === 0 && !collapsed && (
              <div className="px-3 py-4 text-xs text-secondary text-center">
                <Bot className="w-5 h-5 mx-auto mb-2 opacity-50" />
                <p>No agents yet</p>
                <p className="mt-1 opacity-70">
                  Create an agent in ~/.syscity/agents
                </p>
              </div>
            )}
            {agents.map((agent) => (
              <button
                key={agent.id}
                onClick={() => onCreateSessionWithAgent(agent.id)}
                className={`w-full text-left px-3 py-2 rounded-lg text-sm transition flex items-center gap-2 text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04] ${
                  collapsed ? "justify-center" : ""
                }`}
                title={`New session with ${agent.display_name}`}
                role="listitem"
              >
                <span className="text-base shrink-0" aria-hidden="true">
                  {agent.emoji}
                </span>
                {!collapsed && (
                  <>
                    <span className="truncate flex-1 min-w-0">
                      {agent.display_name}
                    </span>
                    {agent.has_heartbeat && (
                      <HeartPulse className="w-3 h-3 text-emerald-500 shrink-0" />
                    )}
                  </>
                )}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Bottom: Network + Theme + Settings */}
      <div className="p-3 shrink-0 border-t border-subtle">
        <div
          className={`flex items-center ${
            collapsed ? "justify-center" : "justify-between"
          }`}
        >
          {!collapsed && (
            <div className="flex items-center gap-2 text-xs text-secondary">
              <StatusDot status={networkStatus} />
              <span className="capitalize">{networkStatus}</span>
            </div>
          )}
          {collapsed && <StatusDot status={networkStatus} />}
          <div className="flex items-center gap-1">
            <button
              onClick={() =>
                setTheme(resolvedTheme === "dark" ? "light" : "dark")
              }
              className="p-1.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary transition"
              title="Toggle theme"
              aria-label="Toggle theme"
            >
              {resolvedTheme === "dark" ? (
                <Sun className="w-4 h-4" />
              ) : (
                <Moon className="w-4 h-4" />
              )}
            </button>
            <button
              onClick={onOpenSettings}
              className="p-1.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary transition"
              title="Settings"
              aria-label="Settings"
            >
              <Settings className="w-4 h-4" />
            </button>
          </div>
        </div>
      </div>
    </aside>
  );
}

interface SessionRowProps {
  session: SessionItem;
  currentSessionId: string;
  collapsed: boolean;
  onSwitch: () => void;
  onRename?: (id: string, name: string) => void | Promise<void>;
  onDelete?: (id: string) => void | Promise<void>;
  onPin?: (id: string, pinned: boolean) => void | Promise<void>;
}

function SessionRow({
  session,
  currentSessionId,
  collapsed,
  onSwitch,
  onRename,
  onDelete,
  onPin,
}: SessionRowProps) {
  const isActive = session.id === currentSessionId;
  const displayName = session.label || session.agent?.display_name || "Untitled";
  const [isEditing, setIsEditing] = useState(false);
  const [editName, setEditName] = useState(displayName);
  const inputRef = useRef<HTMLInputElement>(null);

  const handleRename = useCallback(() => {
    const trimmed = editName.trim();
    if (trimmed && trimmed !== displayName && onRename) {
      onRename(session.id, trimmed);
    }
    setIsEditing(false);
  }, [editName, displayName, onRename, session.id]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Enter") {
        e.preventDefault();
        handleRename();
      } else if (e.key === "Escape") {
        setIsEditing(false);
        setEditName(displayName);
      }
    },
    [handleRename, displayName]
  );

  const handleBlur = useCallback(() => {
    setTimeout(() => {
      if (document.activeElement !== inputRef.current) {
        handleRename();
      }
    }, 150);
  }, [handleRename]);

  const handleDelete = useCallback(() => {
    if (!onDelete) return;
    if (confirm(`Delete session "${displayName}"?`)) {
      onDelete(session.id);
    }
  }, [displayName, onDelete, session.id]);

  const handlePin = useCallback(() => {
    if (!onPin) return;
    onPin(session.id, !session.pinned);
  }, [onPin, session.id, session.pinned]);

  if (collapsed) {
    return (
      <button
        onClick={onSwitch}
        className={`w-full text-left px-3 py-2 rounded-lg text-sm transition flex items-center justify-center ${
          isActive
            ? "bg-primary-100 dark:bg-primary-900/20 text-primary"
            : "text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
        }`}
        title={displayName}
        aria-label={displayName}
        role="listitem"
      >
        <span className="text-base shrink-0" aria-hidden="true">
          {session.agent?.emoji || "💬"}
        </span>
      </button>
    );
  }

  if (isEditing) {
    return (
      <div className="px-3 py-1.5">
        <input
          ref={inputRef}
          type="text"
          value={editName}
          onChange={(e) => setEditName(e.target.value)}
          onKeyDown={handleKeyDown}
          onBlur={handleBlur}
          autoFocus
          className="w-full text-sm px-2 py-1 rounded-md bg-card text-primary border border-subtle focus:outline-none focus:ring-2 focus:ring-primary-500/20"
        />
      </div>
    );
  }

  return (
    <div
      className={`group flex items-center gap-1 px-1 py-0.5 rounded-lg transition ${
        isActive
          ? "bg-primary-100 dark:bg-primary-900/20"
          : "hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
      }`}
      role="listitem"
    >
      <button
        onClick={onSwitch}
        className={`flex-1 min-w-0 text-left px-2 py-1.5 rounded-md text-sm flex items-center gap-2 transition ${
          isActive ? "text-primary" : "text-secondary"
        }`}
        title={displayName}
      >
        <span className="text-base shrink-0" aria-hidden="true">
          {session.agent?.emoji || "💬"}
        </span>
        <span className="truncate flex-1 min-w-0">{displayName}</span>
      </button>
      {(onPin || onRename || onDelete) && (
        <div className="flex items-center gap-0.5 pr-1 opacity-0 group-hover:opacity-100 transition-opacity">
          {onPin && (
            <button
              onClick={handlePin}
              className={`p-1 rounded-md transition ${
                session.pinned
                  ? "text-primary hover:text-primary"
                  : "text-secondary hover:text-primary"
              } hover:bg-black/5 dark:hover:bg-white/5`}
              title={session.pinned ? "Unpin session" : "Pin session"}
              aria-label={session.pinned ? "Unpin session" : "Pin session"}
            >
              {session.pinned ? (
                <PinOff className="w-3 h-3" />
              ) : (
                <Pin className="w-3 h-3" />
              )}
            </button>
          )}
          {onRename && (
            <button
              onClick={() => {
                setEditName(displayName);
                setIsEditing(true);
              }}
              className="p-1 rounded-md text-secondary hover:text-primary hover:bg-black/5 dark:hover:bg-white/5 transition"
              title="Rename session"
              aria-label="Rename session"
            >
              <Pencil className="w-3 h-3" />
            </button>
          )}
          {onDelete && (
            <button
              onClick={handleDelete}
              className="p-1 rounded-md text-secondary hover:text-red-500 hover:bg-red-500/10 transition"
              title="Delete session"
              aria-label="Delete session"
            >
              <Trash2 className="w-3 h-3" />
            </button>
          )}
        </div>
      )}
    </div>
  );
}
