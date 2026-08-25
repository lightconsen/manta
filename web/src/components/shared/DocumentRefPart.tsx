import { FileText, Eye } from "lucide-react";
import { useCallback } from "react";
import { useChatStore } from "@/stores/chatStore";

interface DocumentRefPartProps {
  data: {
    filename: string;
    title: string;
    format: string;
    url?: string;
    export_url?: string;
  };
}

export function DocumentRefPart({ data }: DocumentRefPartProps) {
  const setPreviewDocument = useChatStore((s) => s.setPreviewDocument);

  const handlePreview = useCallback(() => {
    setPreviewDocument({
      filename: data.filename,
      title: data.title,
      format: data.format,
      url: data.url,
      exportUrl: data.export_url,
    });
  }, [data, setPreviewDocument]);

  const formatBadge =
    data.format === "slides"
      ? "PPT"
      : data.format === "docx"
        ? "DOCX"
        : data.format === "xlsx"
          ? "XLSX"
          : data.format === "html"
            ? "HTML"
            : "MD";

  return (
    <div className="my-3 rounded-xl border border-subtle bg-card overflow-hidden">
      <div className="flex items-center gap-3 px-4 py-3">
        <div className="shrink-0 w-9 h-9 rounded-lg bg-primary-100 dark:bg-primary-900/20 flex items-center justify-center">
          <FileText className="w-5 h-5 text-primary-600 dark:text-primary-400" />
        </div>
        <div className="flex-1 min-w-0">
          <div className="text-sm font-medium text-primary truncate">
            {data.title}
          </div>
          <div className="text-[11px] text-secondary mt-0.5">
            <span className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-black/5 dark:bg-white/10">
              {formatBadge}
            </span>
            <span className="ml-2">{data.filename}</span>
          </div>
        </div>
        <button
          type="button"
          onClick={handlePreview}
          className="shrink-0 flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-primary-500 hover:bg-primary-600 text-white transition shadow-sm"
        >
          <Eye className="w-3.5 h-3.5" />
          Preview
        </button>
      </div>
    </div>
  );
}
