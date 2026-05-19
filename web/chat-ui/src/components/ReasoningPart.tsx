import { useState } from "react";

export function ReasoningPart({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="my-2 rounded-lg border border-amber-200 dark:border-amber-800 bg-amber-50 dark:bg-amber-950/30 overflow-hidden">
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium text-amber-700 dark:text-amber-400 hover:bg-amber-100 dark:hover:bg-amber-900/30 transition"
      >
        <svg
          className={`w-3.5 h-3.5 transition-transform ${expanded ? "rotate-90" : ""}`}
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
        </svg>
        <span>Thinking</span>
        <span className="ml-auto text-amber-500 dark:text-amber-600 text-[10px]">
          {expanded ? "Hide" : "Show"}
        </span>
      </button>
      {expanded && (
        <div className="px-3 py-2 text-xs text-amber-800 dark:text-amber-300 font-mono whitespace-pre-wrap leading-relaxed border-t border-amber-200 dark:border-amber-800">
          {text}
        </div>
      )}
    </div>
  );
}
