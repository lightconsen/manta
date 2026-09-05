import { Loader2 } from "lucide-react";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { useChatStore } from "@/stores/chatStore";
import { useEffectiveModel } from "@/hooks/useEffectiveModel";
import { StatusDot } from "@/components/chat/StatusDot";

interface StatusbarProps {
  transport: SyscityWebSocketTransport;
  /** Mirror the sidebar width so the status cluster starts at its right edge. */
  sidebarCollapsed: boolean;
}

/**
 * App-wide bottom bar (hermes-desktop-style shell chrome):
 *
 *   [sidebar-width zone] [left cluster: status items] ...... [right cluster: runtime/model]
 *
 * Pane-following color, mirroring the Titlebar: the leading zone sits on
 * the sidebar surface, the rest on the page surface, so both columns read
 * as full-height panes and the status cluster aligns with the agent
 * identity strip above. Left: connection dot + status word + gateway
 * version. Right: run indicator + effective model for the active session.
 * Context/token usage is intentionally absent — no WS surface exposes it
 * yet (deferred).
 */
export function Statusbar({ transport, sidebarCollapsed }: StatusbarProps) {
  const networkStatus = useChatStore((s) => s.networkStatus);
  const isRunning = useChatStore((s) => s.isRunning);
  // serverInfo is set just before the status flips to "connected", so
  // re-reading it on status change picks up the fresh version.
  const version = networkStatus === "connected" ? transport.getServerInfo().version : undefined;
  const model = useEffectiveModel(transport);

  return (
    <div
      className="h-7 shrink-0 flex items-center pr-3 bg-page border-t border-subtle text-xs text-secondary"
      style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
    >
      {/* Sidebar-width mirror zone (empty; matches the Titlebar's zone) */}
      <div
        className={`hidden md:block shrink-0 self-stretch bg-sidebar transition-all duration-300 ${
          sidebarCollapsed ? "w-16" : "w-64"
        }`}
      />

      {/* Left cluster: status items — starts at the sidebar's right edge */}
      <div className="flex items-center gap-2 min-w-0 pl-3">
        <span className="flex items-center gap-1.5 capitalize">
          <StatusDot status={networkStatus} />
          {networkStatus}
        </span>
        {version && (
          <span className="truncate" title={`Gateway ${version}`}>
            v{version}
          </span>
        )}
      </div>

      {/* Spacer / drag-free gap */}
      <div className="flex-1" />

      {/* Right cluster: runtime / model */}
      <div className="flex items-center gap-3 shrink-0">
        {isRunning && (
          <span className="flex items-center gap-1.5" title="Assistant is running">
            <Loader2 className="w-3 h-3 animate-spin" />
            Running
          </span>
        )}
        {model && <span className="truncate max-w-[16rem]">{model}</span>}
      </div>
    </div>
  );
}
