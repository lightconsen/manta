import { Loader2 } from "lucide-react";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { useChatStore } from "@/stores/chatStore";
import { useEffectiveModel } from "@/hooks/useEffectiveModel";
import { StatusDot } from "@/components/chat/StatusDot";

interface StatusbarProps {
  transport: SyscityWebSocketTransport;
}

/**
 * App-wide bottom bar (hermes-desktop-style shell chrome):
 *
 *   [left cluster: status items] ...... [right cluster: runtime/model]
 *
 * Left: connection dot + status word + gateway version. Right: run
 * indicator + effective model for the active session. Context/token usage
 * is intentionally absent — no WS surface exposes it yet (deferred).
 */
export function Statusbar({ transport }: StatusbarProps) {
  const networkStatus = useChatStore((s) => s.networkStatus);
  const isRunning = useChatStore((s) => s.isRunning);
  // serverInfo is set just before the status flips to "connected", so
  // re-reading it on status change picks up the fresh version.
  const version = networkStatus === "connected" ? transport.getServerInfo().version : undefined;
  const model = useEffectiveModel(transport);

  return (
    <div
      className="h-7 shrink-0 flex items-center justify-between px-3 bg-sidebar border-t border-subtle text-xs text-secondary"
      style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
    >
      {/* Left cluster: status items */}
      <div className="flex items-center gap-2 min-w-0">
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
