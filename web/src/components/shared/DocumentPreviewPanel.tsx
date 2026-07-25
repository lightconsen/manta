import { useState, useEffect, useCallback } from "react";
import {
  X,
  FileText,
  Loader2,
  AlertCircle,
  FolderOpen,
  Share2,
  Download,
  Check,
} from "lucide-react";
import { MarkdownMessage } from "./MarkdownMessage";

interface DocumentPreviewPanelProps {
  document: { filename: string; title: string; format: string };
  onClose: () => void;
}

type LoadState = "loading" | "loaded" | "error";

export function DocumentPreviewPanel({
  document,
  onClose,
}: DocumentPreviewPanelProps) {
  const [content, setContent] = useState<string | null>(null);
  const [loadState, setLoadState] = useState<LoadState>("loading");

  useEffect(() => {
    let cancelled = false;
    setLoadState("loading");
    setContent(null);

    fetch(`/api/v1/artifacts/${document.filename}`)
      .then((res) => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.text();
      })
      .then((text) => {
        if (cancelled) return;
        setContent(text);
        setLoadState("loaded");
      })
      .catch((err) => {
        if (cancelled) return;
        console.error("Failed to load document:", err);
        setLoadState("error");
      });

    return () => {
      cancelled = true;
    };
  }, [document.filename]);

  const [copied, setCopied] = useState(false);
  const isTauri = typeof window !== "undefined" && "__TAURI__" in window;

  const handleOpenFolder = useCallback(async () => {
    if (!isTauri) return;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("reveal_in_folder", { filename: document.filename });
    } catch (err) {
      console.error("Failed to open folder:", err);
    }
  }, [document.filename, isTauri]);

  const handleCopyLink = useCallback(async () => {
    const url = `${window.location.origin}/api/v1/artifacts/${document.filename}`;
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Fallback for environments without clipboard API
      const ta = window.document.createElement("textarea");
      ta.value = url;
      window.document.body.appendChild(ta);
      ta.select();
      window.document.execCommand("copy");
      window.document.body.removeChild(ta);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  }, [document.filename]);

  const handleDownload = useCallback(() => {
    if (!content) return;
    const blob = new Blob([content], {
      type: document.format === "html" ? "text/html" : "text/markdown",
    });
    const objUrl = URL.createObjectURL(blob);
    const a = window.document.createElement("a");
    a.href = objUrl;
    a.download = document.filename;
    a.click();
    URL.revokeObjectURL(objUrl);
  }, [content, document.filename, document.format]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      onClose();
    }
  };

  // Focus trap: auto-focus the panel on mount for ESC key handling
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  return (
    <div
      className="w-[45%] min-w-[400px] max-w-[60%] border-l border-subtle bg-page flex flex-col overflow-hidden"
      role="complementary"
      aria-label="Document preview"
      onKeyDown={handleKeyDown}
    >
      {/* Title bar */}
      <div className="shrink-0 h-14 flex items-center justify-between px-4 border-b border-subtle">
        <div className="flex items-center gap-2 min-w-0">
          <FileText className="w-5 h-5 text-primary-500 shrink-0" />
          <div className="min-w-0">
            <div className="text-sm font-medium text-primary truncate">
              {document.title}
            </div>
            <div className="text-[10px] text-secondary">
              <span className="inline-flex items-center px-1 py-0.5 rounded text-[9px] font-medium bg-black/5 dark:bg-white/10">
                {document.format === "html" ? "HTML" : "MD"}
              </span>
            </div>
          </div>
        </div>
        <div className="flex items-center gap-1">
          {/* Open in folder — Tauri desktop only */}
          {isTauri && (
            <button
              type="button"
              onClick={handleOpenFolder}
              className="p-1.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary hover:text-primary transition"
              title="Open file location"
              aria-label="Open file location"
            >
              <FolderOpen className="w-4 h-4" />
            </button>
          )}
          {/* Download */}
          <button
            type="button"
            onClick={handleDownload}
            disabled={loadState !== "loaded"}
            className="p-1.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary hover:text-primary transition disabled:opacity-30 disabled:pointer-events-none"
            title="Download file"
            aria-label="Download file"
          >
            <Download className="w-4 h-4" />
          </button>
          {/* Copy link / Share */}
          <button
            type="button"
            onClick={handleCopyLink}
            className="p-1.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary hover:text-primary transition"
            title={copied ? "Copied!" : "Copy link"}
            aria-label="Copy link"
          >
            {copied ? (
              <Check className="w-4 h-4 text-green-500" />
            ) : (
              <Share2 className="w-4 h-4" />
            )}
          </button>
          {/* Close */}
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary hover:text-primary transition"
            title="Close preview (Esc)"
            aria-label="Close preview"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
      </div>

      {/* Content area */}
      <div className="flex-1 overflow-y-auto p-4">
        {loadState === "loading" && (
          <div className="flex items-center justify-center h-40 text-secondary">
            <Loader2 className="w-6 h-6 animate-spin" />
            <span className="ml-2 text-sm">Loading document…</span>
          </div>
        )}
        {loadState === "error" && (
          <div className="flex flex-col items-center justify-center h-40 text-secondary gap-2">
            <AlertCircle className="w-8 h-8 text-red-400" />
            <p className="text-sm">Failed to load document</p>
            <p className="text-xs opacity-60">{document.filename}</p>
          </div>
        )}
        {loadState === "loaded" && content !== null && (
          <div className="document-preview-content">
            {document.format === "html" ? (
              <iframe
                className="w-full h-full min-h-[60vh] rounded-lg border border-subtle"
                srcDoc={content}
                sandbox="allow-same-origin"
                title={document.title}
              />
            ) : (
              <div className="prose prose-sm dark:prose-invert max-w-none">
                <MarkdownMessage text={content} />
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
