import { useState } from "react";
import { X } from "lucide-react";
import { LocalKbPanel } from "./LocalKbPanel";
import { BackupSyncPanel } from "./BackupSyncPanel";

export interface KbAgent {
  id: string;
  display_name: string;
  emoji: string;
  is_valid: boolean;
  has_heartbeat: boolean;
}

/** Full-screen Knowledge Base page (replaces the chat area when opened from
 * the sidebar). Two tabs: Local — per-agent collections served by the
 * engine's embedded RAG (`kb-{agent_id}`, immediately retrievable by that
 * agent); Backup — back up those collections to Syscity Cloud and restore
 * them on any device (cloud is storage only, never a retrieval path). */
export function KnowledgeBaseView({ agents, onClose }: { agents: KbAgent[]; onClose: () => void }) {
  const [tab, setTab] = useState<"local" | "backup">("local");

  const tabCls = (active: boolean) =>
    `px-3 py-1.5 rounded-lg text-sm font-medium transition ${
      active
        ? "bg-primary-100 dark:bg-primary-900/30 text-primary"
        : "text-secondary hover:bg-black/5 dark:hover:bg-white/5"
    }`;

  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-page">
      <div className="flex items-center justify-between px-4 md:px-5 py-3 border-b border-subtle shrink-0">
        <h2 className="text-base font-semibold text-primary">Knowledge Base</h2>
        <button
          onClick={onClose}
          className="p-1.5 rounded-lg hover:bg-black/5 dark:hover:bg-white/5 text-secondary transition"
          title="Back to chat"
          aria-label="Back to chat"
        >
          <X className="w-5 h-5" />
        </button>
      </div>
      <div className="px-4 md:px-5 pt-3 shrink-0">
        <div className="inline-flex items-center gap-1 p-1 rounded-xl bg-black/[0.03] dark:bg-white/[0.04]">
          <button className={tabCls(tab === "local")} onClick={() => setTab("local")}>
            Local
          </button>
          <button className={tabCls(tab === "backup")} onClick={() => setTab("backup")}>
            Backup
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto px-4 md:px-5 py-4">
        {tab === "local" ? <LocalKbPanel agents={agents} /> : <BackupSyncPanel agents={agents} />}
      </div>
    </div>
  );
}
