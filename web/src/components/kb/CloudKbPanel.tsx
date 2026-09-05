import { useCallback, useEffect, useRef, useState } from "react";
import { Cloud, FileText, Loader2, Plus, RefreshCw, Search, Trash2, Upload } from "lucide-react";
import { getActiveTransport } from "@/SyscityWebSocketTransport";
import { cloudLoginUrl, cloudStatus, type CloudStatus } from "@/lib/cloud";

interface CloudKb {
  id: string;
  name: string;
  document_count?: number;
  created_at?: string;
}

interface CloudKbHit {
  content: string;
  source?: string;
  score?: number;
}

const LOGIN_TIMEOUT_MS = 180_000;
const POLL_MS = 1_500;
/** Upload cap mirrored from the gateway (`cloud.kb.upload`: 32 MiB). */
const MAX_UPLOAD_BYTES = 32 * 1024 * 1024;

async function fileToBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

/** Cloud knowledge bases: list / create / delete KBs, upload documents and
 * run test-queries. All operations passthrough the gateway to Syscity Cloud
 * (`cloud.kb.*` WS methods); cloud-side indexing may lag uploads — the raw
 * returned status is shown verbatim. */
export function CloudKbPanel() {
  const [status, setStatus] = useState<CloudStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [kbs, setKbs] = useState<CloudKb[]>([]);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  const [busyKb, setBusyKb] = useState<string | null>(null);
  const [uploadNote, setUploadNote] = useState<string | null>(null);
  const [queryKb, setQueryKb] = useState<string | null>(null);
  const [queryText, setQueryText] = useState("");
  const [querying, setQuerying] = useState(false);
  const [queryHits, setQueryHits] = useState<CloudKbHit[] | null>(null);
  const pollTimer = useRef<number | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const st = await cloudStatus();
      setStatus(st);
      if (st.enabled && st.logged_in) {
        const transport = getActiveTransport();
        if (!transport) throw new Error("No gateway connection");
        const body = (await transport.cloudKbList()) as { knowledge_bases?: CloudKb[] } | undefined;
        setKbs(body?.knowledge_bases ?? []);
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
    // Poll cloud.status until the popup stores the token, then load KBs.
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

  const createKb = async () => {
    const name = newName.trim();
    if (!name) return;
    setCreating(true);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      await transport.cloudKbCreate(name);
      setNewName("");
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreating(false);
    }
  };

  const deleteKb = async (kb: CloudKb) => {
    if (!confirm(`Delete knowledge base "${kb.name}"? This cannot be undone.`)) return;
    setBusyKb(kb.id);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      await transport.cloudKbDelete(kb.id);
      if (queryKb === kb.id) {
        setQueryKb(null);
        setQueryHits(null);
      }
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyKb(null);
    }
  };

  const uploadTo = async (kbId: string, files: File[]) => {
    if (files.length === 0) return;
    setError(null);
    setUploadNote(null);
    setBusyKb(`${kbId}:upload`);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      const notes: string[] = [];
      for (const file of files) {
        if (file.size > MAX_UPLOAD_BYTES) {
          notes.push(`${file.name}: exceeds ${MAX_UPLOAD_BYTES / (1024 * 1024)} MB`);
          continue;
        }
        try {
          const base64 = await fileToBase64(file);
          const res = (await transport.cloudKbUpload(kbId, file.name, base64, file.type || undefined)) as
            | { status?: string; filename?: string }
            | undefined;
          notes.push(`${file.name}: ${res?.status ?? "uploaded"}`);
        } catch (e) {
          notes.push(`${file.name}: ${e instanceof Error ? e.message : String(e)}`);
        }
      }
      setUploadNote(notes.join(" · ") || null);
      await load();
    } finally {
      setBusyKb(null);
    }
  };

  const runQuery = async () => {
    if (!queryKb || !queryText.trim()) return;
    setQuerying(true);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      const res = (await transport.cloudKbQuery(queryKb, queryText.trim(), 5)) as
        | { hits?: CloudKbHit[] }
        | undefined;
      setQueryHits(res?.hits ?? []);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setQuerying(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center py-16 text-secondary">
        <Loader2 className="w-5 h-5 animate-spin mr-2" /> Loading cloud knowledge bases…
      </div>
    );
  }

  // Not enabled / not signed in — same guidance as the account control.
  if (!status?.enabled || !status.logged_in) {
    return (
      <div className="max-w-lg mx-auto mt-10 text-center">
        <Cloud className="w-10 h-10 mx-auto mb-3 text-secondary/60" />
        <h3 className="text-sm font-semibold text-primary mb-1">
          {!status?.enabled ? "Cloud is not enabled" : "Sign in to manage cloud knowledge bases"}
        </h3>
        <p className="text-xs text-secondary mb-4">
          {!status?.enabled
            ? "Enable cloud in Settings → General to store knowledge bases in Syscity Cloud."
            : "Cloud knowledge bases live in your Syscity Cloud account and are shared across devices."}
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

  return (
    <div className="max-w-3xl mx-auto space-y-4">
      {error && <div className="text-xs text-red-600 dark:text-red-400">{error}</div>}

      {/* Create */}
      <div className="p-4 rounded-lg bg-card flex flex-col sm:flex-row gap-3 sm:items-end">
        <label className="flex-1">
          <span className="block text-xs text-secondary mb-1">New knowledge base</span>
          <input
            type="text"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && createKb()}
            placeholder="Product docs"
            className="w-full text-sm px-3 py-2 rounded-md bg-page text-primary border border-subtle focus:outline-none focus:ring-2 focus:ring-primary-500/20"
          />
        </label>
        <button
          onClick={createKb}
          disabled={creating || !newName.trim()}
          className="inline-flex items-center gap-1.5 px-4 py-2 rounded-lg text-sm font-medium bg-primary-600 text-white hover:bg-primary-700 transition disabled:opacity-50"
        >
          {creating ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Plus className="w-3.5 h-3.5" />}
          Create
        </button>
      </div>

      <div className="flex items-center justify-between">
        <h3 className="text-xs uppercase tracking-wider text-secondary font-medium">
          Knowledge bases ({kbs.length})
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

      {uploadNote && <div className="text-xs text-secondary bg-card rounded-lg px-3 py-2">{uploadNote}</div>}

      {kbs.length === 0 ? (
        <div className="py-10 text-center text-xs text-secondary bg-card rounded-lg">
          No cloud knowledge bases yet — create one above.
        </div>
      ) : (
        <div className="rounded-lg bg-card divide-y divide-subtle">
          {kbs.map((kb) => (
            <div key={kb.id} className="px-4 py-3">
              <div className="flex items-center gap-3">
                <FileText className="w-4 h-4 shrink-0 text-secondary" />
                <div className="min-w-0 flex-1">
                  <p className="text-sm text-primary truncate">{kb.name}</p>
                  <p className="text-[10px] text-secondary/70">
                    {kb.document_count ?? 0} documents
                    {kb.created_at ? ` · created ${new Date(kb.created_at).toLocaleString()}` : ""}
                  </p>
                </div>
                <label className="cursor-pointer p-1.5 rounded-md text-secondary hover:text-primary hover:bg-black/5 dark:hover:bg-white/5 transition shrink-0" title="Upload document">
                  {busyKb === `${kb.id}:upload` ? (
                    <Loader2 className="w-3.5 h-3.5 animate-spin" />
                  ) : (
                    <Upload className="w-3.5 h-3.5" />
                  )}
                  <input
                    type="file"
                    className="hidden"
                    onChange={(e) => {
                      const files = Array.from(e.target.files ?? []);
                      e.target.value = "";
                      void uploadTo(kb.id, files);
                    }}
                  />
                </label>
                <button
                  onClick={() => {
                    setQueryKb(queryKb === kb.id ? null : kb.id);
                    setQueryHits(null);
                    setQueryText("");
                  }}
                  className={`p-1.5 rounded-md transition shrink-0 ${
                    queryKb === kb.id
                      ? "text-primary bg-primary-50 dark:bg-primary-900/20"
                      : "text-secondary hover:text-primary hover:bg-black/5 dark:hover:bg-white/5"
                  }`}
                  title="Test query"
                  aria-label={`Test query ${kb.name}`}
                >
                  <Search className="w-3.5 h-3.5" />
                </button>
                <button
                  onClick={() => deleteKb(kb)}
                  disabled={busyKb === kb.id}
                  className="p-1.5 rounded-md text-secondary hover:text-red-500 hover:bg-red-500/10 transition disabled:opacity-50 shrink-0"
                  title="Delete knowledge base"
                  aria-label={`Delete ${kb.name}`}
                >
                  {busyKb === kb.id ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Trash2 className="w-3.5 h-3.5" />}
                </button>
              </div>
              {queryKb === kb.id && (
                <div className="mt-3 pl-7 space-y-2">
                  <div className="flex gap-2">
                    <input
                      type="text"
                      value={queryText}
                      onChange={(e) => setQueryText(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && runQuery()}
                      placeholder="Test query…"
                      className="flex-1 text-sm px-3 py-1.5 rounded-md bg-page text-primary border border-subtle focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                    />
                    <button
                      onClick={runQuery}
                      disabled={querying || !queryText.trim()}
                      className="px-3 py-1.5 rounded-md text-xs font-medium bg-primary-600 text-white hover:bg-primary-700 transition disabled:opacity-50"
                    >
                      {querying ? "Searching…" : "Search"}
                    </button>
                  </div>
                  {queryHits !== null &&
                    (queryHits.length === 0 ? (
                      <p className="text-[11px] text-secondary/70">
                        No hits. (Cloud-side indexing may still be processing — documents upload asynchronously.)
                      </p>
                    ) : (
                      <div className="space-y-1.5">
                        {queryHits.map((h, i) => (
                          <div key={i} className="text-[11px] rounded-md bg-page px-2.5 py-2">
                            <p className="text-primary line-clamp-3">{h.content}</p>
                            <p className="text-secondary/70 mt-1">
                              {h.source ? `${h.source} · ` : ""}
                              {h.score !== undefined ? `score ${h.score.toFixed(3)}` : ""}
                            </p>
                          </div>
                        ))}
                      </div>
                    ))}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
