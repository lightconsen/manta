import { useMemo, useState, useEffect, useCallback, useRef } from "react";
import {
  AssistantRuntimeProvider,
  ThreadPrimitive,
  ComposerPrimitive,
  useLocalRuntime,
  useComposerRuntime,
} from "@assistant-ui/react";
import {
  MantaWebSocketTransport,
  type NetworkStatus,
  type ChatMessage,
} from "./MantaWebSocketTransport";
import {
  getCommandCompletions,
  type CommandDef,
  type CommandCategory,
} from "./slash-commands";
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
  onOpenSettings,
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
  onOpenSettings: () => void;
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
              onClick={onOpenSettings}
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

/* ── Live Status Bar ── */

/** Diverse status messages based on tool name and elapsed time. */
function getStatusText(
  status: "thinking" | "tool_calling",
  toolName: string | undefined,
  elapsedSec: number
): string {
  if (status === "thinking") {
    const pool = elapsedSec > 30
      ? ["Still thinking", "Deep in thought", "Processing", "Analyzing"]
      : elapsedSec > 10
        ? ["Thinking", "Pondering", "Reasoning", "Considering"]
        : ["Thinking", "Analyzing", "Reasoning"];
    return pool[Math.floor(elapsedSec / 3) % pool.length];
  }

  // tool_calling — diverse messages per tool
  const name = toolName?.toLowerCase() || "";
  if (name.includes("file_read") || name.includes("fileread")) {
    return "Reading files";
  }
  if (name.includes("file_write") || name.includes("filewrite")) {
    return "Writing files";
  }
  if (name.includes("file_edit") || name.includes("fileedit")) {
    return "Editing files";
  }
  if (name.includes("shell") || name.includes("bash")) {
    const pool = elapsedSec > 15
      ? ["Running command", "Executing shell", "Processing output"]
      : ["Running command", "Executing shell"];
    return pool[Math.floor(elapsedSec / 5) % pool.length];
  }
  if (name.includes("web_search") || name.includes("websearch")) {
    return "Searching the web";
  }
  if (name.includes("heartbeat")) {
    return "Checking heartbeat";
  }
  if (name.includes("cron")) {
    return "Managing scheduled tasks";
  }
  if (name.includes("memory") || name.includes("recall")) {
    return "Recalling memory";
  }
  if (name.includes("upgrade") || name.includes("patch")) {
    return "Applying changes";
  }
  if (name.includes("delegate")) {
    return "Delegating to subagent";
  }
  if (name.includes("browser")) {
    return "Browsing the web";
  }
  if (name.includes("git")) {
    return "Running git";
  }
  if (name.includes("build") || name.includes("cargo")) {
    return "Building project";
  }
  if (name.includes("test")) {
    return "Running tests";
  }

  // Generic tool messages
  const generic = elapsedSec > 20
    ? ["Still working", "Processing", "Running tools"]
    : ["Running tools", "Executing", "Working"];
  return generic[Math.floor(elapsedSec / 4) % generic.length];
}

function LiveStatusBar({
  liveStatus,
  startTime,
}: {
  liveStatus: { status: "thinking" | "tool_calling"; toolName?: string };
  startTime: number;
}) {
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => {
      setElapsed(Date.now() - startTime);
    }, 200);
    return () => clearInterval(interval);
  }, [startTime]);

  const formatElapsed = (ms: number): string => {
    const sec = Math.floor(ms / 1000);
    const min = Math.floor(sec / 60);
    const s = sec % 60;
    if (min > 0) return `${min}:${s.toString().padStart(2, "0")}`;
    return `${s}s`;
  };

  const elapsedSec = Math.floor(elapsed / 1000);
  const statusText = getStatusText(liveStatus.status, liveStatus.toolName, elapsedSec);

  return (
    <div className="mt-1.5 flex items-center gap-2 text-[11px] text-emerald-600 dark:text-emerald-400/80">
      {/* Animated dot */}
      <span className="relative flex h-2 w-2">
        <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75" />
        <span className="relative inline-flex rounded-full h-2 w-2 bg-emerald-500" />
      </span>
      {/* Status text */}
      <span className="font-medium">{statusText}</span>
      {/* Animated dots */}
      <span className="inline-flex w-4">
        <span className="animate-pulse">.</span>
        <span className="animate-pulse" style={{ animationDelay: "150ms" }}>.</span>
        <span className="animate-pulse" style={{ animationDelay: "300ms" }}>.</span>
      </span>
      {/* Elapsed timer */}
      <span className="text-[10px] text-gray-400 dark:text-neutral-500 font-mono tabular-nums">
        {formatElapsed(elapsed)}
      </span>
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
  const isAssistant = message.role === "assistant";
  const hasMetadata = isAssistant && (message.durationMs !== undefined || message.toolCount !== undefined);

  const formatDuration = (ms: number): string => {
    if (ms < 1000) return `${ms}ms`;
    const sec = (ms / 1000).toFixed(1).replace(/\.0$/, '');
    return `${sec}s`;
  };

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
          {/* Summary footer or live status */}
          {message.liveStatus && (
            <LiveStatusBar
              liveStatus={message.liveStatus}
              startTime={message.timestamp ?? Date.now() - 5000}
            />
          )}
          {!message.liveStatus && hasMetadata && (
            <div className="mt-1.5 flex items-center gap-3 text-[10px] text-gray-400 dark:text-neutral-500">
              {message.durationMs !== undefined && (
                <span className="flex items-center gap-1">
                  <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <circle cx="12" cy="12" r="10" strokeWidth="2" />
                    <polyline points="12 6 12 12 16 14" strokeWidth="2" />
                  </svg>
                  {formatDuration(message.durationMs!)}
                </span>
              )}
              {message.toolCount !== undefined && message.toolCount > 0 && (
                <span className="flex items-center gap-1">
                  <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
                    <circle cx="12" cy="12" r="3" strokeWidth="2" />
                  </svg>
                  {message.toolCount} tool{message.toolCount !== 1 ? 's' : ''}
                </span>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/* ── Command Palette ── */
function categoryIcon(category: CommandCategory): string {
  const map: Record<CommandCategory, string> = {
    session: "🗂️",
    model: "🧠",
    status: "ℹ️",
    agents: "🤖",
    tools: "🛠️",
    admin: "🔒",
  };
  return map[category];
}

function CommandPalette({
  commands,
  selectedIndex,
  onSelect,
}: {
  commands: CommandDef[];
  selectedIndex: number;
  onSelect: (cmd: CommandDef) => void;
}) {
  if (commands.length === 0) return null;
  return (
    <div className="absolute bottom-full left-0 right-0 mb-2 bg-white dark:bg-neutral-800 rounded-xl shadow-xl border border-gray-200 dark:border-neutral-700 overflow-hidden z-50">
      <div className="max-h-64 overflow-y-auto">
        {commands.map((cmd, i) => (
          <button
            key={cmd.key}
            type="button"
            onClick={() => onSelect(cmd)}
            onMouseEnter={() => {}}
            className={`w-full text-left px-3 py-2 flex items-center gap-3 transition ${
              i === selectedIndex
                ? "bg-emerald-50 dark:bg-emerald-900/20"
                : "hover:bg-gray-50 dark:hover:bg-neutral-700/50"
            }`}
          >
            <span className="text-base w-5 text-center shrink-0">
              {categoryIcon(cmd.category)}
            </span>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <span className="text-sm font-semibold text-gray-900 dark:text-gray-100">
                  /{cmd.name}
                </span>
                {cmd.args && (
                  <span className="text-xs text-gray-400 dark:text-neutral-500 font-mono">
                    {cmd.args}
                  </span>
                )}
              </div>
              <div className="text-xs text-gray-500 dark:text-neutral-400 truncate">
                {cmd.description}
              </div>
            </div>
            {cmd.local && (
              <span className="text-[10px] px-1.5 py-0.5 rounded bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400 font-medium shrink-0">
                local
              </span>
            )}
            {cmd.requires_admin && (
              <span className="text-[10px] px-1.5 py-0.5 rounded bg-red-100 dark:bg-red-900/30 text-red-600 dark:text-red-400 font-medium shrink-0">
                admin
              </span>
            )}
          </button>
        ))}
      </div>
      <div className="px-3 py-1.5 border-t border-gray-100 dark:border-neutral-700 text-[10px] text-gray-400 dark:text-neutral-500 flex items-center gap-3">
        <span>↑↓ to navigate</span>
        <span>↵ to select</span>
        <span>esc to close</span>
      </div>
    </div>
  );
}

/* ── Chat Content ── */
function ChatContent({ messages, transport }: { messages: ChatMessage[]; transport: MantaWebSocketTransport }) {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const composer = useComposerRuntime();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteIndex, setPaletteIndex] = useState(0);
  const [paletteCommands, setPaletteCommands] = useState<CommandDef[]>([]);
  const [isRunning, setIsRunning] = useState(false);

  useEffect(() => {
    return transport.onRunStateChange(setIsRunning);
  }, [transport]);

  const handleInput = useCallback(() => {
    const val = inputRef.current?.value || "";
    if (val.startsWith("/")) {
      const filter = val.slice(1).split(" ")[0] || "";
      const cmds = getCommandCompletions(filter);
      setPaletteCommands(cmds);
      setPaletteOpen(cmds.length > 0);
      setPaletteIndex(0);
    } else {
      setPaletteOpen(false);
    }
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (!paletteOpen) return;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setPaletteIndex((i) => Math.min(i + 1, paletteCommands.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setPaletteIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        const cmd = paletteCommands[paletteIndex];
        if (cmd) {
          composer.setText(`/${cmd.name} `);
          setPaletteOpen(false);
          setTimeout(() => inputRef.current?.focus(), 0);
        }
      } else if (e.key === "Escape") {
        setPaletteOpen(false);
      }
    },
    [paletteOpen, paletteCommands, paletteIndex, composer]
  );

  const handleSelectCommand = useCallback(
    (cmd: CommandDef) => {
      composer.setText(`/${cmd.name} `);
      setPaletteOpen(false);
      setTimeout(() => inputRef.current?.focus(), 0);
    },
    [composer]
  );

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
        <ComposerPrimitive.Root className="max-w-3xl mx-auto w-full">
          <div className="relative flex flex-col rounded-xl border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 focus-within:ring-2 focus-within:ring-emerald-500/30 focus-within:border-emerald-500/50 transition">
            {/* Command palette */}
            {paletteOpen && (
              <CommandPalette
                commands={paletteCommands}
                selectedIndex={paletteIndex}
                onSelect={handleSelectCommand}
              />
            )}

            {/* Multiline input */}
            <ComposerPrimitive.Input
              ref={inputRef}
              onInput={handleInput}
              onKeyDown={handleKeyDown}
              className="w-full resize-none bg-transparent px-4 pt-3 pb-1 text-sm text-gray-900 dark:text-gray-100 placeholder-gray-400 dark:placeholder-neutral-500 focus:outline-none min-h-[60px] max-h-[200px]"
              placeholder="Message Manta..."
              rows={1}
            />

            {/* Bottom toolbar */}
            <div className="flex items-center justify-between px-2 pb-2 pt-1">
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  title="Voice input"
                  className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-emerald-500 dark:hover:text-emerald-400 hover:bg-gray-100 dark:hover:bg-neutral-700/50 transition"
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
                  className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-emerald-500 dark:hover:text-emerald-400 hover:bg-gray-100 dark:hover:bg-neutral-700/50 transition"
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
                  className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-emerald-500 dark:hover:text-emerald-400 hover:bg-gray-100 dark:hover:bg-neutral-700/50 transition"
                  onClick={() => alert('File upload coming soon')}
                >
                  <svg className="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
                  </svg>
                </button>
              </div>
              {isRunning ? (
                <button
                  type="button"
                  onClick={() => transport.abort()}
                  title="Stop generating"
                  className="shrink-0 p-2 rounded-lg bg-red-500 hover:bg-red-600 text-white transition shadow-sm"
                >
                  <svg className="w-4 h-4" viewBox="0 0 24 24" fill="currentColor">
                    <rect x="6" y="6" width="12" height="12" rx="2" />
                  </svg>
                </button>
              ) : (
                <ComposerPrimitive.Send className="shrink-0 p-2 rounded-lg bg-gradient-to-r from-emerald-500 to-teal-600 hover:from-emerald-600 hover:to-teal-700 disabled:opacity-40 text-white transition shadow-sm">
                  <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                    <line x1="22" y1="2" x2="11" y2="13" />
                    <polygon points="22 2 15 22 11 13 2 9 22 2" />
                  </svg>
                </ComposerPrimitive.Send>
              )}
            </div>
          </div>
        </ComposerPrimitive.Root>
      </div>
    </ThreadPrimitive.Root>
  );
}

/* ── Settings Panel ── */
interface ChannelConfig {
  name: string;
  channel_type: string;
  enabled: boolean;
  agent_id?: string;
  dm_policy?: string;
  require_mention?: boolean;
  has_credentials?: boolean;
}

interface MantaConfig {
  model?: string;
  model_provider?: string;
  default_agent?: {
    temperature?: number;
    max_tokens?: number;
    max_turns?: number | null;
    max_concurrent_tools?: number;
    system_prompt?: string;
    workspace_only?: boolean;
  };
  heartbeat?: {
    enabled?: boolean;
    interval_seconds?: number;
    active_hours_start?: string;
    active_hours_end?: string;
    max_consecutive_idle?: number;
  };
  channels?: ChannelConfig[];
}

function SettingsPanel({
  transport,
  onClose,
}: {
  transport: MantaWebSocketTransport;
  onClose: () => void;
}) {
  const [config, setConfig] = useState<MantaConfig>({});
  const [models, setModels] = useState<Array<{ id: string; name: string; provider: string }>>([]);
  const [agents, setAgents] = useState<string[]>([]);
  const [agentRegistry, setAgentRegistry] = useState<Array<{ id: string; display_name: string; is_valid: boolean; has_heartbeat: boolean }>>([]);
  const [sessions, setSessions] = useState<Array<{ id: string; label?: string }>>([]);
  const [crons, setCrons] = useState<Array<Record<string, unknown>>>([]);
  const [skills, setSkills] = useState<Array<Record<string, unknown>>>([]);
  const [loading, setLoading] = useState(false);
  const [activeTab, setActiveTab] = useState("general");
  const [showAddChannel, setShowAddChannel] = useState(false);
  const [addChannelError, setAddChannelError] = useState("");
  const [newChannel, setNewChannel] = useState({
    name: "",
    channel_type: "telegram",
    enabled: true,
    agent_id: "",
  });
  const [channelActionLoading, setChannelActionLoading] = useState<string>("");
  const [showAddModel, setShowAddModel] = useState(false);
  const [addModelError, setAddModelError] = useState("");
  const [newModel, setNewModel] = useState({ name: "", provider: "anthropic", model: "" });
  const [modelActionLoading, setModelActionLoading] = useState<string>("");

  useEffect(() => {
    setLoading(true);
    Promise.all([
      transport.getConfig(),
      transport.listModels(),
      transport.listAgents(),
      transport.listAgentRegistry(),
      transport.listSessions(),
      transport.listCrons(),
      transport.listSkills(),
    ])
      .then(([cfg, mdl, agt, reg, sess, cronRes, skillRes]) => {
        setConfig(cfg as MantaConfig);
        setModels(mdl.models || []);
        setAgents(agt.agents || []);
        setAgentRegistry(reg.agents || []);
        setSessions(sess || []);
        setCrons(cronRes.jobs || []);
        setSkills(skillRes.skills || []);
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [transport]);

  const update = async (path: string, value: unknown) => {
    const ok = await transport.setConfig(path, value);
    if (ok) {
      setConfig((prev) => {
        const next = { ...prev };
        const parts = path.split(".");
        if (parts.length === 1) {
          (next as Record<string, unknown>)[parts[0]] = value as never;
        } else if (parts.length === 2 && next[parts[0] as keyof MantaConfig]) {
          const section = { ...(next[parts[0] as keyof MantaConfig] as Record<string, unknown>) };
          section[parts[1]] = value;
          (next as Record<string, unknown>)[parts[0]] = section as never;
        }
        return next;
      });
    }
  };

  const da = config.default_agent || {};
  const hb = config.heartbeat || {};
  const channels = config.channels || [];

  const refreshConfig = async () => {
    try {
      const cfg = await transport.getConfig();
      setConfig(cfg as MantaConfig);
    } catch {
      /* ignore */
    }
  };

  const handleAddChannel = async () => {
    setAddChannelError("");
    if (!newChannel.name.trim()) {
      setAddChannelError("Channel name is required");
      return;
    }
    setChannelActionLoading("add");
    const ok = await transport.addChannel({
      name: newChannel.name.trim(),
      channel_type: newChannel.channel_type,
      enabled: newChannel.enabled,
      agent_id: newChannel.agent_id.trim() || undefined,
    });
    if (ok) {
      setNewChannel({ name: "", channel_type: "telegram", enabled: true, agent_id: "" });
      setShowAddChannel(false);
      await refreshConfig();
    } else {
      setAddChannelError("Failed to add channel");
    }
    setChannelActionLoading("");
  };

  const handleRemoveChannel = async (name: string) => {
    if (!confirm(`Remove channel "${name}"?`)) return;
    setChannelActionLoading(name);
    await transport.removeChannel(name);
    await refreshConfig();
    setChannelActionLoading("");
  };

  const handleToggleChannel = async (name: string, enabled: boolean) => {
    setChannelActionLoading(name);
    await transport.setChannelEnabled(name, enabled);
    await refreshConfig();
    setChannelActionLoading("");
  };

  const handleAddModel = async () => {
    setAddModelError("");
    if (!newModel.name.trim()) {
      setAddModelError("Model alias is required");
      return;
    }
    if (!newModel.model.trim()) {
      setAddModelError("Model ID is required");
      return;
    }
    setModelActionLoading("add");
    const ok = await transport.addModel({
      name: newModel.name.trim(),
      provider: newModel.provider,
      model: newModel.model.trim(),
    });
    if (ok) {
      setNewModel({ name: "", provider: "anthropic", model: "" });
      setShowAddModel(false);
      await refreshModels();
    } else {
      setAddModelError("Failed to add model");
    }
    setModelActionLoading("");
  };

  const handleRemoveModel = async (name: string) => {
    if (!confirm(`Remove model "${name}"?`)) return;
    setModelActionLoading(name);
    await transport.removeModel(name);
    await refreshModels();
    setModelActionLoading("");
  };

  const handleSetDefaultModel = async (name: string) => {
    setModelActionLoading(`default_${name}`);
    const ok = await transport.setDefaultModel(name);
    if (ok) {
      await refreshModels();
      setConfig((prev) => ({ ...prev, model: name }));
    }
    setModelActionLoading("");
  };

  const refreshModels = async () => {
    try {
      const mdl = await transport.listModels();
      setModels(mdl.models || []);
    } catch {
      /* ignore */
    }
  };

  const tabs = [
    { id: "general", label: "General" },
    { id: "channels", label: "Channels" },
    { id: "models", label: "Models" },
    { id: "agents", label: "Agents" },
    { id: "jobs", label: "Jobs" },
    { id: "sessions", label: "Sessions" },
    { id: "skills", label: "Skills" },
    { id: "logs", label: "Logs" },
  ];

  const tabCls = (id: string) =>
    `w-full text-left px-3 py-1.5 rounded-md text-sm transition ${
      activeTab === id
        ? "bg-emerald-50 dark:bg-emerald-900/20 text-emerald-700 dark:text-emerald-400 font-medium"
        : "text-gray-600 dark:text-neutral-400 hover:bg-gray-100 dark:hover:bg-neutral-800"
    }`;

  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-white dark:bg-neutral-900">
      {/* Header */}
      <div className="flex items-center justify-between px-5 py-3 border-b border-gray-200 dark:border-neutral-800 shrink-0">
        <h2 className="text-base font-semibold text-gray-900 dark:text-gray-100">Settings</h2>
        <button
          onClick={onClose}
          className="p-1.5 rounded-lg hover:bg-gray-100 dark:hover:bg-neutral-800 text-gray-400 dark:text-neutral-400 transition"
          title="Back to chat"
        >
          <svg className="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>

      {loading ? (
        <div className="flex-1 flex items-center justify-center text-gray-400 dark:text-neutral-500">
          <div className="w-6 h-6 border-2 border-gray-200 dark:border-neutral-600 border-t-emerald-500 rounded-full animate-spin mb-3 mr-3" />
          Loading configuration...
        </div>
      ) : (
        <div className="flex-1 flex overflow-hidden">
          {/* Left vertical tabs */}
          <div className="w-44 border-r border-gray-200 dark:border-neutral-800 shrink-0 overflow-y-auto py-3 px-2 space-y-0.5">
            {tabs.map((t) => (
              <button key={t.id} onClick={() => setActiveTab(t.id)} className={tabCls(t.id)}>
                {t.label}
              </button>
            ))}
          </div>

          {/* Right content */}
          <div className="flex-1 overflow-y-auto px-5 py-4">
            {activeTab === "general" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Gateway</h3>
                  <div className="space-y-2">
                    {(() => {
                      const si = transport.getServerInfo();
                      return (
                        <>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">URL</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-mono">{transport.getGatewayUrl() || "—"}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Version</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-mono">{si.version || "—"}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Connection</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-mono">{si.conn_id || "—"}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Features</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100">{(si.features || []).join(", ") || "—"}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Auth Mode</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-mono capitalize">{String((config as Record<string, unknown>).auth_mode || "—")}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Scopes</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100">{(si.scopes_granted || []).join(", ") || "—"}</span>
                          </div>
                        </>
                      );
                    })()}
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Model & Provider</h3>
                  <div className="space-y-3">
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Default Model</label>
                      <select value={config.model || ""} onChange={(e) => update("model", e.target.value)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30">
                        {models.map((m) => <option key={m.id} value={m.id}>{m.name} ({m.provider})</option>)}
                      </select>
                    </div>
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Provider</label>
                      <input type="text" value={config.model_provider || ""} readOnly className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-100 dark:bg-neutral-800 px-3 py-2 text-sm text-gray-500 dark:text-neutral-400 cursor-not-allowed" />
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Appearance</h3>
                  <div className="space-y-3">
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Theme Mode</label>
                      <div className="flex gap-2">
                        {(["system", "light", "dark"] as const).map((m) => (
                          <button key={m} onClick={() => { localStorage.setItem("manta-theme", m); document.documentElement.classList.toggle("dark", m === "dark" || (m === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches)); }} className="px-3 py-1.5 rounded-lg border border-gray-200 dark:border-neutral-600 text-sm text-gray-700 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-neutral-800 transition capitalize">
                            {m}
                          </button>
                        ))}
                      </div>
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Heartbeat</h3>
                  <div className="space-y-3">
                    <div className="flex items-center justify-between">
                      <label className="text-sm text-gray-700 dark:text-gray-300">Enable Heartbeat</label>
                      <button onClick={() => update("heartbeat.enabled", !hb.enabled)} className={`relative inline-flex h-5 w-9 items-center rounded-full transition ${hb.enabled ? "bg-emerald-500" : "bg-gray-300 dark:bg-neutral-600"}`}>
                        <span className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition ${hb.enabled ? "translate-x-4.5" : "translate-x-0.5"}`} />
                      </button>
                    </div>
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Interval (seconds)</label>
                      <input type="number" value={hb.interval_seconds ?? 300} onChange={(e) => update("heartbeat.interval_seconds", parseInt(e.target.value))} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30" />
                    </div>
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Active From</label>
                        <input type="text" value={hb.active_hours_start || ""} onChange={(e) => update("heartbeat.active_hours_start", e.target.value)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30" />
                      </div>
                      <div>
                        <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Active To</label>
                        <input type="text" value={hb.active_hours_end || ""} onChange={(e) => update("heartbeat.active_hours_end", e.target.value)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30" />
                      </div>
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Token Usage</h3>
                  <div className="text-sm text-gray-500 dark:text-neutral-400">Token usage tracking coming soon.</div>
                </section>
              </div>
            )}

            {activeTab === "channels" && (
              <div className="space-y-5">
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider">Configured Channels</h3>
                    <button
                      onClick={() => { setShowAddChannel(!showAddChannel); setAddChannelError(""); }}
                      className="px-3 py-1 rounded-md bg-emerald-500 hover:bg-emerald-600 text-white text-xs font-medium transition"
                    >
                      {showAddChannel ? "Cancel" : "+ Add"}
                    </button>
                  </div>

                  {showAddChannel && (
                    <div className="mb-4 p-4 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 space-y-3">
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Name</label>
                          <input
                            type="text"
                            value={newChannel.name}
                            onChange={(e) => setNewChannel({ ...newChannel, name: e.target.value })}
                            placeholder="my-bot"
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30"
                          />
                        </div>
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Type</label>
                          <select
                            value={newChannel.channel_type}
                            onChange={(e) => setNewChannel({ ...newChannel, channel_type: e.target.value })}
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30"
                          >
                            <option value="telegram">Telegram</option>
                            <option value="discord">Discord</option>
                            <option value="slack">Slack</option>
                            <option value="whatsapp">WhatsApp</option>
                            <option value="qq">QQ</option>
                            <option value="feishu">Feishu</option>
                            <option value="signal">Signal</option>
                            <option value="imessage">iMessage</option>
                            <option value="webchat">WebChat</option>
                            <option value="websocket">WebSocket</option>
                            <option value="web_terminal">Web Terminal</option>
                          </select>
                        </div>
                      </div>
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Agent ID (optional)</label>
                          <input
                            type="text"
                            value={newChannel.agent_id}
                            onChange={(e) => setNewChannel({ ...newChannel, agent_id: e.target.value })}
                            placeholder="default"
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30"
                          />
                        </div>
                        <div className="flex items-center gap-2 pt-5">
                          <input
                            id="ch-enabled"
                            type="checkbox"
                            checked={newChannel.enabled}
                            onChange={(e) => setNewChannel({ ...newChannel, enabled: e.target.checked })}
                            className="rounded border-gray-300 text-emerald-500 focus:ring-emerald-500"
                          />
                          <label htmlFor="ch-enabled" className="text-sm text-gray-700 dark:text-gray-300">Enabled</label>
                        </div>
                      </div>
                      {addChannelError && (
                        <div className="text-xs text-red-600 dark:text-red-400">{addChannelError}</div>
                      )}
                      <div className="flex justify-end">
                        <button
                          onClick={handleAddChannel}
                          disabled={channelActionLoading === "add"}
                          className="px-4 py-1.5 rounded-md bg-emerald-500 hover:bg-emerald-600 disabled:opacity-50 text-white text-xs font-medium transition"
                        >
                          {channelActionLoading === "add" ? "Adding..." : "Add Channel"}
                        </button>
                      </div>
                    </div>
                  )}

                  {channels.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No channels configured.</div>
                  ) : (
                    <div className="space-y-2">
                      {channels.map((ch) => (
                        <div key={ch.name} className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <div className="flex items-center gap-3">
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{ch.name}</span>
                            <span className="text-xs px-1.5 py-0.5 rounded bg-gray-200 dark:bg-neutral-700 text-gray-600 dark:text-neutral-400 uppercase">{ch.channel_type}</span>
                            {ch.agent_id && (
                              <span className="text-xs text-gray-500 dark:text-neutral-400 font-mono">{ch.agent_id}</span>
                            )}
                            {ch.dm_policy && ch.dm_policy !== "open" && (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-400">{ch.dm_policy}</span>
                            )}
                            {ch.require_mention && (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-400">mention</span>
                            )}
                            {ch.has_credentials && (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400">auth</span>
                            )}
                          </div>
                          <div className="flex items-center gap-2">
                            <button
                              onClick={() => handleToggleChannel(ch.name, !ch.enabled)}
                              disabled={channelActionLoading === ch.name}
                              className={`text-xs px-2 py-0.5 rounded-full transition ${
                                ch.enabled
                                  ? "bg-emerald-100 dark:bg-emerald-900/30 text-emerald-700 dark:text-emerald-400 hover:bg-emerald-200 dark:hover:bg-emerald-900/50"
                                  : "bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400 hover:bg-gray-200 dark:hover:bg-neutral-600"
                              }`}
                            >
                              {channelActionLoading === ch.name ? "..." : ch.enabled ? "Enabled" : "Disabled"}
                            </button>
                            <button
                              onClick={() => handleRemoveChannel(ch.name)}
                              disabled={channelActionLoading === ch.name}
                              className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-gray-400 dark:text-neutral-400 hover:text-red-600 dark:hover:text-red-400 transition"
                              title="Remove"
                            >
                              <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                <line x1="18" y1="6" x2="6" y2="18" />
                                <line x1="6" y1="6" x2="18" y2="18" />
                              </svg>
                            </button>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "models" && (
              <div className="space-y-5">
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider">Available Models</h3>
                    <button
                      onClick={() => { setShowAddModel(!showAddModel); setAddModelError(""); }}
                      className="px-3 py-1 rounded-md bg-emerald-500 hover:bg-emerald-600 text-white text-xs font-medium transition"
                    >
                      {showAddModel ? "Cancel" : "+ Add"}
                    </button>
                  </div>

                  {showAddModel && (
                    <div className="mb-4 p-4 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 space-y-3">
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Alias</label>
                          <input
                            type="text"
                            value={newModel.name}
                            onChange={(e) => setNewModel({ ...newModel, name: e.target.value })}
                            placeholder="smart"
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30"
                          />
                        </div>
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Provider</label>
                          <select
                            value={newModel.provider}
                            onChange={(e) => setNewModel({ ...newModel, provider: e.target.value })}
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30"
                          >
                            <option value="anthropic">Anthropic</option>
                            <option value="openai">OpenAI</option>
                            <option value="deepseek">DeepSeek</option>
                            <option value="gemini">Gemini</option>
                            <option value="qwen">Qwen</option>
                          </select>
                        </div>
                      </div>
                      <div>
                        <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Model ID</label>
                        <input
                          type="text"
                          value={newModel.model}
                          onChange={(e) => setNewModel({ ...newModel, model: e.target.value })}
                          placeholder="claude-3-5-sonnet-20241022"
                          className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30"
                        />
                      </div>
                      {addModelError && (
                        <div className="text-xs text-red-600 dark:text-red-400">{addModelError}</div>
                      )}
                      <div className="flex justify-end">
                        <button
                          onClick={handleAddModel}
                          disabled={modelActionLoading === "add"}
                          className="px-4 py-1.5 rounded-md bg-emerald-500 hover:bg-emerald-600 disabled:opacity-50 text-white text-xs font-medium transition"
                        >
                          {modelActionLoading === "add" ? "Adding..." : "Add Model"}
                        </button>
                      </div>
                    </div>
                  )}

                  {models.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No models available.</div>
                  ) : (
                    <div className="space-y-2">
                      {models.map((m) => (
                        <div key={m.id} className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <div className="flex items-center gap-2">
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{m.name}</span>
                            <span className="text-xs text-gray-500 dark:text-neutral-400">{m.provider}</span>
                          </div>
                          <div className="flex items-center gap-2">
                            {config.model === m.id ? (
                              <span className="text-xs px-2 py-0.5 rounded-full bg-emerald-100 dark:bg-emerald-900/30 text-emerald-700 dark:text-emerald-400">Default</span>
                            ) : (
                              <button
                                onClick={() => handleSetDefaultModel(m.id)}
                                disabled={modelActionLoading === `default_${m.id}`}
                                className="text-xs px-2 py-0.5 rounded-full bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400 hover:bg-emerald-100 dark:hover:bg-emerald-900/30 hover:text-emerald-700 dark:hover:text-emerald-400 transition"
                              >
                                {modelActionLoading === `default_${m.id}` ? "..." : "Set Default"}
                              </button>
                            )}
                            <button
                              onClick={() => handleRemoveModel(m.id)}
                              disabled={modelActionLoading === m.id}
                              className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-gray-400 dark:text-neutral-400 hover:text-red-600 dark:hover:text-red-400 transition"
                              title="Remove"
                            >
                              <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                <line x1="18" y1="6" x2="6" y2="18" />
                                <line x1="6" y1="6" x2="18" y2="18" />
                              </svg>
                            </button>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "agents" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Registered Agents ({agentRegistry.length})</h3>
                  {agentRegistry.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No agents in registry.</div>
                  ) : (
                    <div className="space-y-2">
                      {agentRegistry.map((a) => (
                        <div key={a.id} className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <div className="flex items-center gap-2">
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{a.display_name || a.id}</span>
                            <span className="text-xs text-gray-500 dark:text-neutral-400 font-mono">{a.id}</span>
                          </div>
                          <div className="flex items-center gap-1.5">
                            {a.has_heartbeat && (
                              <span className="text-xs px-2 py-0.5 rounded-full bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400">Heartbeat</span>
                            )}
                            {agents.includes(a.id) ? (
                              <span className="text-xs px-2 py-0.5 rounded-full bg-emerald-100 dark:bg-emerald-900/30 text-emerald-700 dark:text-emerald-400">Running</span>
                            ) : (
                              <span className="text-xs px-2 py-0.5 rounded-full bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400">Stopped</span>
                            )}
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Agent Parameters</h3>
                  <div className="space-y-3">
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Temperature</label>
                        <div className="flex items-center gap-2">
                          <input type="range" min="0" max="2" step="0.1" value={da.temperature ?? 0.7} onChange={(e) => update("default_agent.temperature", parseFloat(e.target.value))} className="flex-1 h-1.5 bg-gray-200 dark:bg-neutral-600 rounded-lg appearance-none cursor-pointer accent-emerald-500" />
                          <span className="text-sm text-gray-600 dark:text-neutral-400 w-10 text-right tabular-nums">{da.temperature ?? 0.7}</span>
                        </div>
                      </div>
                      <div>
                        <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Max Tokens</label>
                        <input type="number" value={da.max_tokens ?? 2048} onChange={(e) => update("default_agent.max_tokens", parseInt(e.target.value))} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30" />
                      </div>
                    </div>
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Max Turns</label>
                        <input type="number" value={da.max_turns ?? ""} placeholder="Unlimited" onChange={(e) => update("default_agent.max_turns", e.target.value ? parseInt(e.target.value) : null)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30" />
                      </div>
                      <div>
                        <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Max Concurrent Tools</label>
                        <input type="number" value={da.max_concurrent_tools ?? 5} onChange={(e) => update("default_agent.max_concurrent_tools", parseInt(e.target.value))} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30" />
                      </div>
                    </div>
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">System Prompt</label>
                      <textarea rows={6} value={da.system_prompt || ""} onChange={(e) => update("default_agent.system_prompt", e.target.value)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/30 resize-none font-mono" />
                    </div>
                  </div>
                </section>
              </div>
            )}

            {activeTab === "jobs" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Jobs ({crons.length})</h3>
                  {crons.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No cron jobs configured.</div>
                  ) : (
                    <div className="space-y-2">
                      {crons.map((job, i) => (
                        <div key={i} className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <div className="flex items-center justify-between">
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{(job as Record<string, string>).name || "Unnamed"}</span>
                            <span className={`text-xs px-2 py-0.5 rounded-full ${job.enabled ? "bg-emerald-100 dark:bg-emerald-900/30 text-emerald-700 dark:text-emerald-400" : "bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400"}`}>
                              {job.enabled ? "Enabled" : "Disabled"}
                            </span>
                          </div>
                          {(job as Record<string, string>).schedule && (
                            <div className="text-xs text-gray-500 dark:text-neutral-400 mt-1 font-mono">{(job as Record<string, string>).schedule}</div>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "sessions" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Active Sessions</h3>
                  {sessions.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No sessions.</div>
                  ) : (
                    <div className="space-y-1">
                      {sessions.map((s) => (
                        <div key={s.id} className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <span className="text-sm text-gray-900 dark:text-gray-100 font-mono">{s.id}</span>
                          {s.label && <span className="text-xs text-gray-500 dark:text-neutral-400">{s.label}</span>}
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "skills" && (
              <div className="space-y-5">
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider">Skills ({skills.length})</h3>
                    <button className="px-3 py-1 rounded-md bg-emerald-500 hover:bg-emerald-600 text-white text-xs font-medium transition">
                      + Install
                    </button>
                  </div>
                  {skills.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No skills loaded.</div>
                  ) : (
                    <div className="space-y-2">
                      {skills.map((s, i) => (
                        <div key={i} className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <div className="flex items-center justify-between">
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{(s as Record<string, string>).name || "Unnamed"}</span>
                            <span className="text-xs text-gray-500 dark:text-neutral-400">{(s as Record<string, string>).version || ""}</span>
                          </div>
                          {(s as Record<string, string>).description && (
                            <div className="text-xs text-gray-500 dark:text-neutral-400 mt-1">{(s as Record<string, string>).description}</div>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "logs" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Logs</h3>
                  <div className="text-sm text-gray-500 dark:text-neutral-400">Log viewer coming soon.</div>
                </section>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/* ── App ── */
function ChatAppInner({ transport }: { transport: MantaWebSocketTransport }) {
  const runtime = useLocalRuntime(transport);
  const [messages, setMessages] = useState<ChatMessage[]>([]);

  useEffect(() => {
    let cancelled = false;
    const doLoad = async () => {
      try {
        const history = await transport.loadHistory(transport.getSessionId());
        if (cancelled) return;
        transport.setMessages(history);
        setMessages(history);
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
      setMessages([...msgs]);
    });
    return () => {
      cancelled = true;
      unsub();
      unsubStatus();
    };
  }, [transport]);

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ChatContent messages={messages} transport={transport} />
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
  const [settingsOpen, setSettingsOpen] = useState(false);

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
        const msg: import("./MantaWebSocketTransport").ChatMessage = {
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
        theme={theme}
        onToggleTheme={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}
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
