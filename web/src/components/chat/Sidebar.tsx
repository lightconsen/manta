import { ChevronLeft, ChevronRight, Plus, Sun, Moon, Settings } from "lucide-react";
import { useThemeStore } from "@/stores/themeStore";
import { StatusDot } from "./StatusDot";
import type { NetworkStatus } from "@/SyscityWebSocketTransport";

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
  sessions: Array<{ id: string; label?: string }>;
  currentSessionId: string;
  onSwitchSession: (id: string) => void;
  onNewSession: () => void;
  networkStatus: NetworkStatus;
  onOpenSettings: () => void;
}

export function Sidebar({
  collapsed,
  onToggle,
  sessions,
  currentSessionId,
  onSwitchSession,
  onNewSession,
  networkStatus,
  onOpenSettings,
}: SidebarProps) {
  const { resolvedTheme, setTheme } = useThemeStore();

  return (
    <aside
      className={`shrink-0 flex flex-col bg-sidebar transition-all duration-300 ${
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

      {/* Middle: Session List */}
      <div className="flex-1 overflow-y-auto py-2" role="list">
        {!collapsed && sessions.length > 0 && (
          <div className="px-3 pb-1">
            <span className="text-[10px] uppercase tracking-wider text-secondary font-medium">
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
                ? "bg-black/[0.04] dark:bg-white/[0.06] text-primary-600 dark:text-primary-400"
                : "text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
            } ${collapsed ? "justify-center" : ""}`}
            title={s.id}
            role="listitem"
          >
            {!collapsed && (
              <span className="truncate">{s.label || s.id}</span>
            )}
            {collapsed && (
              <span className="text-[10px] font-medium truncate max-w-[2.5rem]">
                {(s.label || s.id).slice(0, 3)}
              </span>
            )}
          </button>
        ))}
        <button
          onClick={onNewSession}
          className={`w-full text-left px-3 py-2 mx-1 mt-1 rounded-lg text-sm transition flex items-center gap-2 text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04] ${
            collapsed ? "justify-center" : ""
          }`}
          title="New session"
          aria-label="New session"
        >
          <Plus className="w-4 h-4 shrink-0" />
          {!collapsed && <span>New Session</span>}
        </button>
      </div>

      {/* Bottom: Network + Theme + Settings */}
      <div className="p-3 shrink-0">
        <div className={`flex items-center ${collapsed ? "justify-center" : "justify-between"}`}>
          {!collapsed && (
            <div className="flex items-center gap-2 text-xs text-secondary">
              <StatusDot status={networkStatus} />
              <span className="capitalize">{networkStatus}</span>
            </div>
          )}
          {collapsed && <StatusDot status={networkStatus} />}
          <div className="flex items-center gap-1">
            <button
              onClick={() => setTheme(resolvedTheme === "dark" ? "light" : "dark")}
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
