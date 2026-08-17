import { useEffect, useState } from "react";
import { FolderTree, X } from "lucide-react";
import { useChatStore } from "@/stores/chatStore";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { FileTree } from "./FileTree";
import { FilePreview } from "./FilePreview";

interface WorkspacePanelProps {
  transport: SyscityWebSocketTransport;
  onClose: () => void;
}

/**
 * Right-side panel browsing the workspace of the current session's agent.
 * Keyed by agent id at the render site, so tree/selection state resets on
 * session switch. Two internal views: file tree ↔ single-file preview.
 */
export function WorkspacePanel({ transport, onClose }: WorkspacePanelProps) {
  const currentAgent = useChatStore((s) => s.currentAgent);
  const agentId = currentAgent?.id;
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [root, setRoot] = useState("");

  // Esc closes the panel (same convention as DocumentPreviewPanel).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  const title = currentAgent
    ? `${currentAgent.emoji} ${currentAgent.display_name}`
    : "Default agent";

  return (
    <div
      className="flex-1 min-w-0 border-l border-subtle bg-page flex flex-col overflow-hidden"
      role="complementary"
      aria-label="Workspace files"
    >
      {/* Title bar */}
      <div className="shrink-0 h-14 flex items-center justify-between px-4 border-b border-subtle">
        <div className="flex items-center gap-2 min-w-0">
          <FolderTree className="w-5 h-5 text-primary-500 shrink-0" />
          <div className="min-w-0">
            <div className="text-sm font-medium text-primary truncate">
              {title} · Workspace
            </div>
            <div className="text-[10px] text-secondary truncate" title={root}>
              {selectedPath ?? root}
            </div>
          </div>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="p-1.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary hover:text-primary transition"
          title="Close workspace browser (Esc)"
          aria-label="Close workspace browser"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      {selectedPath ? (
        <FilePreview
          transport={transport}
          agentId={agentId}
          path={selectedPath}
          onBack={() => setSelectedPath(null)}
        />
      ) : (
        <FileTree
          transport={transport}
          agentId={agentId}
          selectedPath={selectedPath}
          onSelectFile={setSelectedPath}
          onRootResolved={setRoot}
        />
      )}
    </div>
  );
}
