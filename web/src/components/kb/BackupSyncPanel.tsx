import { useCallback, useEffect, useRef, useState } from "react";
import { Cloud, HardDriveDownload, HardDriveUpload, Loader2, RefreshCw, Trash2 } from "lucide-react";
import { getActiveTransport } from "@/SyscityWebSocketTransport";
import { cloudLoginUrl, cloudStatus, type CloudStatus } from "@/lib/cloud";
import type { KbAgent } from "./KnowledgeBaseView";

interface CollectionSummary {
  collection: string;
  total_docs: number;
  total_chunks: number;
}

interface CloudKb {
  id: string;
  name: string;
  document_count?: number;
  created_at?: string;
}

interface PushResult {
  total: number;
  pushed: number;
  unchanged: number;
  skipped_url: number;
  skipped_external: number;
  too_large: number;
  failed: number;
  errors: string[];
}

interface PullResult {
  total: number;
  pulled: number;
  unchanged: number;
  failed: number;
  errors: string[];
}

const LOGIN_TIMEOUT_MS = 180_000;
const POLL_MS = 1_500;

/** Display owner for a collection: the agent's name for `kb-{agent_id}`,
 * "Default" for collections not bound to an agent (shared). */
function ownerOf(collection: string, agents: KbAgent[]): { label: string; emoji: string } {
  if (collection.startsWith("kb-") && collection.length > 3) {
    const id = collection.slice(3);
    const a = agents.find((x) => x.id === id);
    if (a) return { label: a.display_name, emoji: a.emoji };
    return { label: id, emoji: "🤖" };
  }
  return { label: "Default", emoji: "📁" };
}

/** Human summary of a push/pull response. */
function resultSummary(r: PushResult | PullResult, verb: string): string {
  const parts = [`${verb} ${"pushed" in r ? r.pushed : r.pulled}`, `${r.unchanged} unchanged`];
  if ("skipped_url" in r && r.skipped_url > 0) parts.push(`${r.skipped_url} url`);
  if ("skipped_external" in r && r.skipped_external > 0) parts.push(`${r.skipped_external} external`);
  if ("too_large" in r && r.too_large > 0) parts.push(`${r.too_large} too large`);
  if (r.failed > 0) parts.push(`${r.failed} failed`);
  if (r.errors.length > 0) parts.push(r.errors[0]);
  return parts.join(" · ");
}

/** Backup & Sync: local collections (the only truth + retrieval path) can be
 * backed up to Syscity Cloud; a backup can be restored into a local agent
 * collection on any device signed into the same account. The cloud stores
 * document bytes only — indexing/retrieval stays local. */
export function BackupSyncPanel({ agents }: { agents: KbAgent[] }) {
  const [status, setStatus] = useState<CloudStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [collections, setCollections] = useState<CollectionSummary[]>([]);
  const [kbs, setKbs] = useState<CloudKb[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [pushNotes, setPushNotes] = useState<Record<string, string>>({});
  const [pullNotes, setPullNotes] = useState<Record<string, string>>({});
  const [pullAgent, setPullAgent] = useState<Record<string, string>>({});
  const pollTimer = useRef<number | null>(null);

  const load = useCallback(async () => {
    setError(null);
    try {
      const st = await cloudStatus();
      setStatus(st);
      if (st.enabled && st.logged_in) {
        const transport = getActiveTransport();
        if (!transport) throw new Error("No gateway connection");
        const [kbBody, colBody] = await Promise.all([
          transport.cloudKbList() as Promise<{ knowledge_bases?: CloudKb[] } | undefined>,
          transport.listKbCollections(),
        ]);
        setKbs(kbBody?.knowledge_bases ?? []);
        const col = colBody as { configured?: boolean; collections?: unknown[] } | undefined;
        setCollections(
          col?.configured ? ((col.collections ?? []) as unknown as CollectionSummary[]) : []
        );
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
    return () => {
      if (pollTimer.current) window.clearInterval(pollTimer.current);
    };
  }, [load]);

  const signIn = () => {
    window.open(cloudLoginUrl("github"), "_blank");
    if (pollTimer.current) window.clearInterval(pollTimer.current);
    const started = Date.now();
    pollTimer.current = window.setInterval(async () => {
      try {
        const st = await cloudStatus();
        if (st.logged_in) {
          if (pollTimer.current) window.clearInterval(pollTimer.current);
          await load();
        }
      } catch {
        /* transient — keep polling */
      }
      if (Date.now() - started > LOGIN_TIMEOUT_MS && pollTimer.current) {
        window.clearInterval(pollTimer.current);
      }
    }, POLL_MS);
  };

  const push = async (collection: string) => {
    setBusy(`push:${collection}`);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      const r = (await transport.cloudKbPush(collection)) as PushResult;
      setPushNotes((prev) => ({ ...prev, [collection]: resultSummary(r, `${r.pushed} pushed`) }));
      await load();
    } catch (e) {
      setPushNotes((prev) => ({
        ...prev,
        [collection]: e instanceof Error ? e.message : String(e),
      }));
    } finally {
      setBusy(null);
    }
  };

  const pull = async (kb: CloudKb, agentId: string) => {
    if (!agentId) return;
    setBusy(`pull:${kb.id}`);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      const r = (await transport.cloudKbPull({ cloud_kb_id: kb.id, agent_id: agentId })) as PullResult;
      setPullNotes((prev) => ({ ...prev, [kb.id]: resultSummary(r, `${r.pulled} restored`) }));
      await load();
    } catch (e) {
      setPullNotes((prev) => ({
        ...prev,
        [kb.id]: e instanceof Error ? e.message : String(e),
      }));
    } finally {
      setBusy(null);
    }
  };

  const deleteKb = async (kb: CloudKb) => {
    if (!confirm(`Delete the cloud backup "${kb.name}"? The local collection is not affected.`)) return;
    setBusy(`del:${kb.id}`);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      await transport.cloudKbDelete(kb.id);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center py-16 text-secondary">
        <Loader2 className="w-5 h-5 animate-spin mr-2" /> Loading backup status…
      </div>
    );
  }

  // Not enabled / not signed in — same guidance as the account control.
  if (!status?.enabled || !status.logged_in) {
    return (
      <div className="max-w-lg mx-auto mt-10 text-center">
        <Cloud className="w-10 h-10 mx-auto mb-3 text-secondary/60" />
        <h3 className="text-sm font-semibold text-primary mb-1">
          {!status?.enabled ? "Cloud is not enabled" : "Sign in to back up your knowledge bases"}
        </h3>
        <p className="text-xs text-secondary mb-4">
          {!status?.enabled
            ? "Enable cloud in Settings → General to back up local collections to Syscity Cloud."
            : "Cloud backups live in your Syscity Cloud account and can be restored on any device."}
        </p>
        {status?.enabled && (
          <button
            onClick={signIn}
            className="px-4 py-2 rounded-lg text-sm font-medium bg-primary-600 text-white hover:bg-primary-700 transition"
          >
            Sign in
          </button>
        )}
      </div>
    );
  }

  const cloudByName = new Map(kbs.map((k) => [k.name, k]));

  return (
    <div className="max-w-3xl mx-auto space-y-4">
      {error && <div className="text-xs text-red-600 dark:text-red-400">{error}</div>}

      <p className="text-xs text-secondary bg-card rounded-lg px-3 py-2">
        Local collections are the single source of truth — agents always retrieve from them. Back
        up stores document bytes in your cloud account; Restore re-ingests a backup into a local
        collection on this device.
      </p>

      {/* Local collections → cloud backups */}
      <div className="flex items-center justify-between">
        <h3 className="text-xs uppercase tracking-wider text-secondary font-medium">
          Local collections
        </h3>
        <button
          onClick={load}
          className="p-1 rounded-md text-secondary hover:text-primary hover:bg-black/5 dark:hover:bg-white/5 transition"
          title="Refresh"
          aria-label="Refresh"
        >
          <RefreshCw className="w-3.5 h-3.5" />
        </button>
      </div>
      <div className="rounded-lg bg-card divide-y divide-subtle">
        {collections.length === 0 ? (
          <div className="py-6 text-center text-xs text-secondary">
            No local collections yet — upload documents in the Local tab first.
          </div>
        ) : (
          collections.map((c) => {
            const owner = ownerOf(c.collection, agents);
            const backup = cloudByName.get(c.collection);
            const countMismatch =
              backup && backup.document_count !== undefined && backup.document_count !== c.total_docs;
            return (
              <div key={c.collection} className="px-4 py-2.5">
                <div className="flex items-center gap-3">
                  <span className="text-base shrink-0" aria-hidden="true">
                    {owner.emoji}
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="text-sm text-primary truncate">{owner.label}</p>
                    <p className="text-[10px] text-secondary/70 truncate">
                      {c.collection} · {c.total_docs} docs
                    </p>
                  </div>
                  {backup ? (
                    <span
                      className={`hidden sm:inline-flex px-2 py-0.5 rounded-full text-[10px] font-medium shrink-0 ${
                        countMismatch
                          ? "bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400"
                          : "bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400"
                      }`}
                      title={countMismatch ? "Document counts differ — push to re-sync" : "Backup is up to date"}
                    >
                      {countMismatch
                        ? `${backup.document_count} on cloud · ${c.total_docs} local`
                        : `Backed up · ${backup.document_count ?? 0} docs`}
                    </span>
                  ) : (
                    <span className="hidden sm:inline-flex px-2 py-0.5 rounded-full text-[10px] bg-black/5 dark:bg-white/10 text-secondary shrink-0">
                      Not backed up
                    </span>
                  )}
                  <button
                    onClick={() => push(c.collection)}
                    disabled={busy !== null}
                    className="inline-flex items-center gap-1 px-2.5 py-1.5 rounded-md text-xs font-medium bg-primary-600 text-white hover:bg-primary-700 transition disabled:opacity-50 shrink-0"
                    title={`Back up ${c.collection} to the cloud`}
                  >
                    {busy === `push:${c.collection}` ? (
                      <Loader2 className="w-3.5 h-3.5 animate-spin" />
                    ) : (
                      <HardDriveUpload className="w-3.5 h-3.5" />
                    )}
                    Back up
                  </button>
                </div>
                {pushNotes[c.collection] && (
                  <p className="mt-1 pl-7 text-[11px] text-secondary/80 break-all">{pushNotes[c.collection]}</p>
                )}
              </div>
            );
          })
        )}
      </div>

      {/* Cloud backups → restore into local collections */}
      <h3 className="text-xs uppercase tracking-wider text-secondary font-medium">Cloud backups</h3>
      <div className="rounded-lg bg-card divide-y divide-subtle">
        {kbs.length === 0 ? (
          <div className="py-6 text-center text-xs text-secondary">
            No cloud backups yet — back up a collection above.
          </div>
        ) : (
          kbs.map((kb) => {
            // One-click restore when the backup name maps to a local agent.
            const mappedAgent = kb.name.startsWith("kb-")
              ? agents.find((a) => a.id === kb.name.slice(3))
              : undefined;
            const chosenAgent = pullAgent[kb.id] ?? "";
            return (
              <div key={kb.id} className="px-4 py-2.5">
                <div className="flex flex-wrap items-center gap-3">
                  <Cloud className="w-4 h-4 shrink-0 text-secondary" />
                  <div className="min-w-0 flex-1">
                    <p className="text-sm text-primary truncate">{kb.name}</p>
                    <p className="text-[10px] text-secondary/70">
                      {kb.document_count ?? 0} documents
                      {kb.created_at ? ` · ${new Date(kb.created_at).toLocaleString()}` : ""}
                    </p>
                  </div>
                  {mappedAgent ? (
                    <button
                      onClick={() => pull(kb, mappedAgent.id)}
                      disabled={busy !== null}
                      className="inline-flex items-center gap-1 px-2.5 py-1.5 rounded-md text-xs font-medium bg-primary-600 text-white hover:bg-primary-700 transition disabled:opacity-50 shrink-0"
                      title={`Restore into ${mappedAgent.display_name}'s collection`}
                    >
                      {busy === `pull:${kb.id}` ? (
                        <Loader2 className="w-3.5 h-3.5 animate-spin" />
                      ) : (
                        <HardDriveDownload className="w-3.5 h-3.5" />
                      )}
                      Restore to {mappedAgent.display_name}
                    </button>
                  ) : (
                    <>
                      <select
                        value={chosenAgent}
                        onChange={(e) => setPullAgent((prev) => ({ ...prev, [kb.id]: e.target.value }))}
                        className="text-xs px-2 py-1.5 rounded-md bg-page text-primary border border-subtle focus:outline-none focus:ring-2 focus:ring-primary-500/20 max-w-36"
                      >
                        <option value="">Restore to…</option>
                        {agents.map((a) => (
                          <option key={a.id} value={a.id}>
                            {a.emoji} {a.display_name}
                          </option>
                        ))}
                      </select>
                      <button
                        onClick={() => pull(kb, chosenAgent)}
                        disabled={busy !== null || !chosenAgent}
                        className="inline-flex items-center gap-1 px-2.5 py-1.5 rounded-md text-xs font-medium bg-primary-600 text-white hover:bg-primary-700 transition disabled:opacity-50 shrink-0"
                      >
                        {busy === `pull:${kb.id}` ? (
                          <Loader2 className="w-3.5 h-3.5 animate-spin" />
                        ) : (
                          <HardDriveDownload className="w-3.5 h-3.5" />
                        )}
                        Restore
                      </button>
                    </>
                  )}
                  <button
                    onClick={() => deleteKb(kb)}
                    disabled={busy !== null}
                    className="p-1.5 rounded-md text-secondary hover:text-red-500 hover:bg-red-500/10 transition disabled:opacity-50 shrink-0"
                    title="Delete cloud backup"
                    aria-label={`Delete backup ${kb.name}`}
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
                {pullNotes[kb.id] && (
                  <p className="mt-1 pl-7 text-[11px] text-secondary/80 break-all">{pullNotes[kb.id]}</p>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
