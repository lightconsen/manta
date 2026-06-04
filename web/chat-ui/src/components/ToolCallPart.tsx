import { useState } from "react";
import { MarkdownMessage } from "./MarkdownMessage";

interface ToolCallPartProps {
  toolName: string;
  args: Record<string, unknown>;
  result?: unknown;
  isError?: boolean;
}

/** Heuristic: does this string look like markdown content? */
function looksLikeMarkdown(text: string): boolean {
  const markdownPatterns = [
    /^\s*#{1,6}\s+/m, // headers
    /!\[.*?\]\(.*?\)/, // images
    /\[.*?\]\(.*?\)/, // links
    /(\*\*|__)(?=\S)(.*?\S)\1/m, // bold
    /(\*|_)(?=\S)(.*?\S)\1/m, // italic
    /^\s*[-*+]\s+/m, // lists
    /^\s*```/m, // code blocks
    /^\s*>\s+/m, // blockquote
    /\|.*\|.*\|/, // tables
  ];
  return markdownPatterns.some((re) => re.test(text));
}

export function ToolCallPart({ toolName, args, result, isError }: ToolCallPartProps) {
  const [expanded, setExpanded] = useState(true);

  const statusColor = isError
    ? "border-red-200 dark:border-red-800 bg-red-50 dark:bg-red-950/30 text-red-700 dark:text-red-400"
    : result !== undefined
    ? "border-green-200 dark:border-green-800 bg-green-50 dark:bg-green-950/30 text-green-700 dark:text-green-400"
    : "border-blue-200 dark:border-blue-800 bg-blue-50 dark:bg-blue-950/30 text-blue-700 dark:text-blue-400";

  const statusText = isError
    ? "Error"
    : result !== undefined
    ? "Done"
    : "Running";

  const resultString =
    typeof result === "string" ? result : result !== undefined ? JSON.stringify(result, null, 2) : undefined;
  const renderAsMarkdown = typeof result === "string" && looksLikeMarkdown(resultString || "");

  return (
    <div className={`my-2 rounded-lg border overflow-hidden ${statusColor}`}>
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium hover:opacity-80 transition"
      >
        <svg
          className={`w-3.5 h-3.5 transition-transform ${expanded ? "rotate-90" : ""}`}
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
        </svg>
        <span className="font-mono">{toolName}</span>
        <span className="ml-auto flex items-center gap-1.5 text-[10px] opacity-70">
          {result === undefined && !isError && (
            <span className="relative flex h-2 w-2">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75" />
              <span className="relative inline-flex rounded-full h-2 w-2 bg-blue-500" />
            </span>
          )}
          {statusText}
        </span>
      </button>
      {expanded && (
        <div className="px-3 py-2 text-xs border-t border-inherit border-opacity-50">
          <div className="mb-2">
            <div className="text-[10px] font-semibold uppercase tracking-wider opacity-60 mb-1">Arguments</div>
            <pre className="bg-black/5 dark:bg-white/5 rounded p-2 overflow-x-auto max-w-full whitespace-pre-wrap font-mono text-[11px]">
              {JSON.stringify(args, null, 2)}
            </pre>
          </div>
          {result !== undefined && (
            <div>
              <div className="text-[10px] font-semibold uppercase tracking-wider opacity-60 mb-1">Result</div>
              {renderAsMarkdown ? (
                <div className="bg-black/5 dark:bg-white/5 rounded p-2 overflow-x-auto max-w-full">
                  <MarkdownMessage text={resultString || ""} />
                </div>
              ) : (
                <pre className="bg-black/5 dark:bg-white/5 rounded p-2 overflow-x-auto max-w-full whitespace-pre-wrap font-mono text-[11px]">
                  {resultString}
                </pre>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
