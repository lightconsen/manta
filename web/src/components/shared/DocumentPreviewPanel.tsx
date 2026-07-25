import { useState, useEffect } from "react";
import { X, FileText, Loader2, AlertCircle } from "lucide-react";
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
