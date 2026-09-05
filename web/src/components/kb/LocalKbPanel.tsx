import { useCallback, useEffect, useRef, useState } from "react";
import { FileText, Loader2, RefreshCw, Trash2 } from "lucide-react";
import { getActiveTransport } from "@/SyscityWebSocketTransport";
import type { KbAgent } from "./KnowledgeBaseView";

interface CollectionSummary {
  collection: string;
  total_docs: number;
  total_chunks: number;
  last_indexed_at: string | null;
  stale_count: number;
  failed_count: number;
}

interface KbDoc {
  doc_id: string;
  source_id: string;
  chunk_count: number;
  status: string;
  error: string | null;
  indexed_at: string;
}

/** One flat row across all collections, with the owning collection attached. */
type DocRow = KbDoc & { collection: string };

/** Upload cap mirrored from the gateway (`kb.ingest`: 32 MiB of raw bytes). */
const MAX_UPLOAD_BYTES = 32 * 1024 * 1024;
const ALLOWED_EXT = ["pdf", "docx", "xlsx", "txt", "md"];

const statusBadge = (status: string) => {
  switch (status) {
    case "indexed":
      return "bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400";
    case "stale":
      return "bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400";
    case "failed":
      return "bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-400";
    default:
      return "bg-black/5 dark:bg-white/10 text-secondary";
  }
};

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

/** File → base64 (same approach as AddSkillForm: no chunking, small files). */
async function fileToBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

/** Local knowledge bases: collections served by the engine's embedded RAG —
 * per-agent (`kb-{agent_id}`, retrievable by that agent) plus unbound
 * "Default" collections. All documents are listed flat with an Agent column;
 * uploads pick their destination via the "Upload to" selector. */
export function LocalKbPanel({ agents }: { agents: KbAgent[] }) {
  const [configured, setConfigured] = useState<boolean | null>(null);
  const [reason, setReason] = useState<string | null>(null);
  const [collections, setCollections] = useState<CollectionSummary[]>([]);
  const [rows, setRows] = useState<DocRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyDoc, setBusyDoc] = useState<string | null>(null);
  const [uploadAgent, setUploadAgent] = useState<string>(agents[0]?.id ?? "");
  const [uploading, setUploading] = useState(false);
  const [uploadNote, setUploadNote] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      const body = await transport.listKbCollections();
      setConfigured(body.configured);
      setReason(body.reason);
      const cols = body.collections as unknown as CollectionSummary[];
      setCollections(cols);
      // Fetch every collection's docs and flatten (Agent column carries the
      // ownership info, so no per-collection drill-down is needed).
      const results = await Promise.all(cols.map((c) => transport.listKbDocs(c.collection)));
      setRows(
        results.flatMap((r, i) =>
          (r.docs as unknown as KbDoc[]).map((d) => ({ ...d, collection: cols[i].collection }))
        )
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const uploadFiles = async (files: FileList | null) => {
    if (!files || !uploadAgent) return;
    setError(null);
    setUploadNote(null);
    setUploading(true);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      const notes: string[] = [];
      for (const file of Array.from(files)) {
        const ext = file.name.split(".").pop()?.toLowerCase() ?? "";
        if (!ALLOWED_EXT.includes(ext)) {
          notes.push(`${file.name}: unsupported type (.${ext})`);
          continue;
        }
        if (file.size > MAX_UPLOAD_BYTES) {
          notes.push(`${file.name}: exceeds ${MAX_UPLOAD_BYTES / (1024 * 1024)} MB`);
          continue;
        }
        try {
          const base64 = await fileToBase64(file);
          await transport.ingestKbDoc(uploadAgent, file.name, base64);
        } catch (e) {
          notes.push(`${file.name}: ${e instanceof Error ? e.message : String(e)}`);
        }
      }
      if (notes.length > 0) setUploadNote(notes.join(" · "));
      await load();
    } finally {
      setUploading(false);
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  };

  const deleteDoc = async (row: DocRow) => {
    if (!confirm(`Delete document "${row.doc_id}" from ${row.collection}?`)) return;
    setBusyDoc(`${row.collection}:${row.doc_id}`);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      await transport.deleteKbDoc(row.collection, row.doc_id);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyDoc(null);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center py-16 text-secondary">
        <Loader2 className="w-5 h-5 animate-spin mr-2" /> Loading knowledge bases…
      </div>
    );
  }

  // Default install state: no API embedding provider configured. Guide the
  // user instead of showing an empty manager.
  if (configured === false) {
    return (
      <div className="max-w-lg mx-auto mt-10 text-center">
        <FileText className="w-10 h-10 mx-auto mb-3 text-secondary/60" />
        <h3 className="text-sm font-semibold text-primary mb-1">Local knowledge base is not configured</h3>
        <p className="text-xs text-secondary mb-3">{reason ?? "Embedding provider is not available."}</p>
        <div className="text-xs text-secondary bg-card rounded-lg p-3 text-left">
          <p className="mb-1">In <code className="px-1 rounded bg-black/5 dark:bg-white/10">~/.syscity/config.toml</code>:</p>
          <pre className="text-[11px] whitespace-pre-wrap">{`[vector_memory]
provider = "openai"
embedding_api_key = "sk-..."`}</pre>
        </div>
      </div>
    );
  }

  const totalChunks = collections.reduce((n, c) => n + c.total_chunks, 0);

  return (
    <div className="max-w-3xl mx-auto space-y-4">
      {error && <div className="text-xs text-red-600 dark:text-red-400">{error}</div>}

      {/* Upload destination + file picker */}
      <div className="p-4 rounded-lg bg-card space-y-3">
        <div className="flex flex-col sm:flex-row gap-3 sm:items-end">
          <label className="flex-1">
            <span className="block text-xs text-secondary mb-1">Upload to</span>
            <select
              value={uploadAgent}
              onChange={(e) => setUploadAgent(e.target.value)}
              className="w-full text-sm px-3 py-2 rounded-md bg-page text-primary border border-subtle focus:outline-none focus:ring-2 focus:ring-primary-500/20"
            >
              {agents.length === 0 && <option value="">No agents available</option>}
              {agents.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.emoji} {a.display_name} ({a.id})
                </option>
              ))}
            </select>
          </label>
          <div>
            <span className="block text-xs text-secondary mb-1 opacity-0 select-none" aria-hidden="true">
              upload
            </span>
            <input
              ref={fileInputRef}
              type="file"
              multiple
              accept={ALLOWED_EXT.map((e) => `.${e}`).join(",")}
              onChange={(e) => uploadFiles(e.target.files)}
              disabled={!uploadAgent || uploading}
              className="w-full text-sm text-secondary file:mr-3 file:py-2 file:px-3 file:rounded-md file:border-0 file:text-xs file:font-medium file:bg-primary-50 file:text-primary-700 dark:file:bg-primary-900/20 dark:file:text-primary-400 hover:file:bg-primary-100 disabled:opacity-50"
            />
          </div>
        </div>
        <p className="text-[10px] text-secondary/70">
          Documents upload to <code className="px-1 rounded bg-black/5 dark:bg-white/10">kb-{uploadAgent || "{agent_id}"}</code> and are retrievable by that agent immediately. Allowed: {ALLOWED_EXT.map((e) => `.${e}`).join(" ")}, up to {MAX_UPLOAD_BYTES / (1024 * 1024)} MB each.
        </p>
        {uploading && (
          <div className="flex items-center gap-2 text-xs text-secondary">
            <Loader2 className="w-3.5 h-3.5 animate-spin" /> Uploading and indexing…
          </div>
        )}
        {uploadNote && <div className="text-xs text-amber-600 dark:text-amber-400">{uploadNote}</div>}
      </div>

      {/* All documents across collections */}
      <div className="flex items-center justify-between">
        <h3 className="text-xs uppercase tracking-wider text-secondary font-medium">
          Documents ({rows.length} in {collections.length} collections · {totalChunks} chunks)
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
        {rows.length === 0 ? (
          <div className="py-8 text-center text-xs text-secondary">
            No documents yet — upload a file above.
          </div>
        ) : (
          rows.map((d) => {
            const owner = ownerOf(d.collection, agents);
            return (
              <div key={`${d.collection}:${d.doc_id}`} className="flex items-center gap-3 px-4 py-2.5">
                <FileText className="w-4 h-4 shrink-0 text-secondary" />
                <div className="min-w-0 flex-1">
                  <p className="text-sm text-primary truncate" title={d.source_id}>
                    {d.doc_id}
                  </p>
                  <p className="text-[10px] text-secondary/70 truncate">
                    {d.chunk_count} chunks · {new Date(d.indexed_at).toLocaleString()}
                    {d.error ? ` · ${d.error}` : ""}
                  </p>
                </div>
                <span
                  className="hidden sm:inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-black/5 dark:bg-white/10 text-[10px] text-secondary shrink-0"
                  title={`Collection ${d.collection}`}
                >
                  <span aria-hidden="true">{owner.emoji}</span>
                  {owner.label}
                </span>
                <span className={`px-2 py-0.5 rounded-full text-[10px] font-medium shrink-0 ${statusBadge(d.status)}`}>
                  {d.status}
                </span>
                <button
                  onClick={() => deleteDoc(d)}
                  disabled={busyDoc === `${d.collection}:${d.doc_id}`}
                  className="p-1.5 rounded-md text-secondary hover:text-red-500 hover:bg-red-500/10 transition disabled:opacity-50 shrink-0"
                  title="Delete document"
                  aria-label={`Delete ${d.doc_id}`}
                >
                  {busyDoc === `${d.collection}:${d.doc_id}` ? (
                    <Loader2 className="w-3.5 h-3.5 animate-spin" />
                  ) : (
                    <Trash2 className="w-3.5 h-3.5" />
                  )}
                </button>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
