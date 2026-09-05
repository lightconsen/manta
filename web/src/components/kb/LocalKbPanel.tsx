import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, FileText, Loader2, RefreshCw, Trash2 } from "lucide-react";
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

/** File → base64 (same approach as AddSkillForm: no chunking, small files). */
async function fileToBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

/** Local knowledge bases: one collection per agent (`kb-{agent_id}`) served
 * by the engine's embedded RAG. Documents uploaded here are immediately
 * retrievable by that agent via its memory context. */
export function LocalKbPanel({ agents }: { agents: KbAgent[] }) {
  const [configured, setConfigured] = useState<boolean | null>(null);
  const [reason, setReason] = useState<string | null>(null);
  const [collections, setCollections] = useState<CollectionSummary[]>([]);
  const [selectedAgent, setSelectedAgent] = useState<string>("");
  const [docs, setDocs] = useState<KbDoc[]>([]);
  const [loading, setLoading] = useState(true);
  const [docsLoading, setDocsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busyDoc, setBusyDoc] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [uploadNote, setUploadNote] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const collection = selectedAgent ? `kb-${selectedAgent}` : "";

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      const body = await transport.listKbCollections();
      setConfigured(body.configured);
      setReason(body.reason);
      setCollections(body.collections as unknown as CollectionSummary[]);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadDocs = useCallback(async (col: string) => {
    if (!col) {
      setDocs([]);
      return;
    }
    setDocsLoading(true);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      const body = await transport.listKbDocs(col);
      setDocs(body.docs as unknown as KbDoc[]);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDocsLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    loadDocs(collection);
  }, [collection, loadDocs]);

  const uploadFiles = async (files: FileList | null) => {
    if (!files || !selectedAgent) return;
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
          await transport.ingestKbDoc(selectedAgent, file.name, base64);
        } catch (e) {
          notes.push(`${file.name}: ${e instanceof Error ? e.message : String(e)}`);
        }
      }
      if (notes.length > 0) setUploadNote(notes.join(" · "));
      await Promise.all([load(), loadDocs(collection)]);
    } finally {
      setUploading(false);
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  };

  const deleteDoc = async (docId: string) => {
    if (!confirm(`Delete document "${docId}" from ${collection}?`)) return;
    setBusyDoc(docId);
    setError(null);
    try {
      const transport = getActiveTransport();
      if (!transport) throw new Error("No gateway connection");
      await transport.deleteKbDoc(collection, docId);
      await Promise.all([load(), loadDocs(collection)]);
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

  const summary = collections.find((c) => c.collection === collection);
  const otherCollections = collections.filter((c) => c.collection !== collection);

  return (
    <div className="max-w-3xl mx-auto space-y-4">
      {error && <div className="text-xs text-red-600 dark:text-red-400">{error}</div>}

      {/* Agent (collection) selector + upload */}
      <div className="p-4 rounded-lg bg-card space-y-3">
        <div className="flex flex-col sm:flex-row gap-3 sm:items-end">
          <label className="flex-1">
            <span className="block text-xs text-secondary mb-1">Agent knowledge base</span>
            <select
              value={selectedAgent}
              onChange={(e) => setSelectedAgent(e.target.value)}
              className="w-full text-sm px-3 py-2 rounded-md bg-page text-primary border border-subtle focus:outline-none focus:ring-2 focus:ring-primary-500/20"
            >
              <option value="">Select an agent…</option>
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
              disabled={!selectedAgent || uploading}
              className="w-full text-sm text-secondary file:mr-3 file:py-2 file:px-3 file:rounded-md file:border-0 file:text-xs file:font-medium file:bg-primary-50 file:text-primary-700 dark:file:bg-primary-900/20 dark:file:text-primary-400 hover:file:bg-primary-100 disabled:opacity-50"
            />
          </div>
        </div>
        <p className="text-[10px] text-secondary/70">
          Files upload to this agent's collection (<code className="px-1 rounded bg-black/5 dark:bg-white/10">{collection || "kb-{agent_id}"}</code>) and are retrievable by that agent immediately. Allowed: {ALLOWED_EXT.map((e) => `.${e}`).join(" ")}, up to {MAX_UPLOAD_BYTES / (1024 * 1024)} MB each.
        </p>
        {uploading && (
          <div className="flex items-center gap-2 text-xs text-secondary">
            <Loader2 className="w-3.5 h-3.5 animate-spin" /> Uploading and indexing…
          </div>
        )}
        {uploadNote && <div className="text-xs text-amber-600 dark:text-amber-400">{uploadNote}</div>}
      </div>

      {/* Selected collection stats */}
      {selectedAgent && summary && (
        <div className="p-4 rounded-lg bg-card">
          <div className="flex items-center justify-between mb-2">
            <h3 className="text-sm font-semibold text-primary">{summary.collection}</h3>
            <button
              onClick={() => Promise.all([load(), loadDocs(collection)])}
              className="p-1 rounded-md text-secondary hover:text-primary hover:bg-black/5 dark:hover:bg-white/5 transition"
              title="Refresh"
              aria-label="Refresh"
            >
              <RefreshCw className="w-3.5 h-3.5" />
            </button>
          </div>
          <div className="flex flex-wrap gap-2 text-[11px]">
            <span className="px-2 py-0.5 rounded-full bg-black/5 dark:bg-white/10 text-secondary">
              {summary.total_docs} docs
            </span>
            <span className="px-2 py-0.5 rounded-full bg-black/5 dark:bg-white/10 text-secondary">
              {summary.total_chunks} chunks
            </span>
            {summary.stale_count > 0 && (
              <span className="px-2 py-0.5 rounded-full bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400">
                {summary.stale_count} stale
              </span>
            )}
            {summary.failed_count > 0 && (
              <span className="px-2 py-0.5 rounded-full bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-400">
                {summary.failed_count} failed
              </span>
            )}
            {summary.last_indexed_at && (
              <span className="px-2 py-0.5 rounded-full bg-black/5 dark:bg-white/10 text-secondary">
                last indexed {new Date(summary.last_indexed_at).toLocaleString()}
              </span>
            )}
          </div>
        </div>
      )}

      {/* Documents of the selected collection */}
      {selectedAgent && (
        <div className="rounded-lg bg-card divide-y divide-subtle">
          {docsLoading ? (
            <div className="flex items-center justify-center py-8 text-secondary">
              <Loader2 className="w-4 h-4 animate-spin mr-2" /> Loading documents…
            </div>
          ) : docs.length === 0 ? (
            <div className="py-8 text-center text-xs text-secondary">
              No documents yet — upload a file above.
            </div>
          ) : (
            docs.map((d) => (
              <div key={d.doc_id} className="flex items-center gap-3 px-4 py-2.5">
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
                <span className={`px-2 py-0.5 rounded-full text-[10px] font-medium shrink-0 ${statusBadge(d.status)}`}>
                  {d.status}
                </span>
                <button
                  onClick={() => deleteDoc(d.doc_id)}
                  disabled={busyDoc === d.doc_id}
                  className="p-1.5 rounded-md text-secondary hover:text-red-500 hover:bg-red-500/10 transition disabled:opacity-50 shrink-0"
                  title="Delete document"
                  aria-label={`Delete ${d.doc_id}`}
                >
                  {busyDoc === d.doc_id ? (
                    <Loader2 className="w-3.5 h-3.5 animate-spin" />
                  ) : (
                    <Trash2 className="w-3.5 h-3.5" />
                  )}
                </button>
              </div>
            ))
          )}
        </div>
      )}

      {/* Other agent collections (not selectable here — switch the agent above) */}
      {otherCollections.length > 0 && (
        <div className="text-[11px] text-secondary/70">
          <span className="inline-flex items-center gap-1 mr-2">
            <CheckCircle2 className="w-3 h-3" /> Other collections:
          </span>
          {otherCollections.map((c) => (
            <button
              key={c.collection}
              onClick={() => setSelectedAgent(c.collection.replace(/^kb-/, ""))}
              className="mr-2 underline decoration-dotted hover:text-primary"
            >
              {c.collection} ({c.total_docs})
            </button>
          ))}
        </div>
      )}

      {!selectedAgent && collections.length > 0 && (
        <div className="flex items-start gap-2 text-[11px] text-secondary/70">
          <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
          Select an agent above to manage its documents.
        </div>
      )}
    </div>
  );
}
