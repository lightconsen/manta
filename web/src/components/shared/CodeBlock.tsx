import { useMemo, useState, useCallback } from "react";
import { useShikiHighlighter } from "@/hooks/useShikiHighlighter";
import { useThemeStore } from "@/stores/themeStore";
import { Check, Copy } from "lucide-react";

interface CodeBlockProps {
  code: string;
  language?: string;
}

export function CodeBlock({ code, language = "text" }: CodeBlockProps) {
  const highlighter = useShikiHighlighter();
  const resolvedTheme = useThemeStore((s) => s.resolvedTheme);
  const [copied, setCopied] = useState(false);

  const html = useMemo(() => {
    if (!highlighter) return null;
    const themeName = resolvedTheme === "dark" ? "github-dark" : "github-light";
    try {
      return highlighter.codeToHtml(code, {
        lang: language === "text" || !language ? "text" : language,
        theme: themeName,
      });
    } catch {
      return highlighter.codeToHtml(code, {
        lang: "text",
        theme: themeName,
      });
    }
  }, [highlighter, code, language, resolvedTheme]);

  const handleCopy = useCallback(() => {
    navigator.clipboard.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  }, [code]);

  if (!html) {
    return (
      <div className="relative rounded-xl border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800/80 overflow-hidden my-3">
        <div className="flex items-center justify-between px-4 py-2 border-b border-gray-200 dark:border-neutral-700 bg-gray-100 dark:bg-neutral-800">
          <span className="text-[10px] font-medium text-gray-500 dark:text-neutral-400 uppercase tracking-wider">
            {language}
          </span>
        </div>
        <pre className="p-4 overflow-x-auto text-xs font-mono leading-relaxed text-gray-800 dark:text-gray-200">
          <code>{code}</code>
        </pre>
      </div>
    );
  }

  return (
    <div className="relative rounded-xl border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800/80 overflow-hidden my-3 group">
      <div className="flex items-center justify-between px-4 py-2 border-b border-gray-200 dark:border-neutral-700 bg-gray-100 dark:bg-neutral-800">
        <span className="text-[10px] font-medium text-gray-500 dark:text-neutral-400 uppercase tracking-wider">
          {language}
        </span>
        <button
          onClick={handleCopy}
          className="flex items-center gap-1 px-2 py-1 rounded-md text-[10px] text-gray-500 dark:text-neutral-400 hover:bg-gray-200 dark:hover:bg-neutral-700 transition opacity-0 group-hover:opacity-100 focus:opacity-100"
          aria-label="Copy code"
          title="Copy"
        >
          {copied ? (
            <>
              <Check className="w-3 h-3" />
              <span>Copied</span>
            </>
          ) : (
            <>
              <Copy className="w-3 h-3" />
              <span>Copy</span>
            </>
          )}
        </button>
      </div>
      <div
        className="p-4 overflow-x-auto text-xs leading-relaxed [&>pre]:!bg-transparent [&>pre]:!p-0 [&>pre]:!m-0"
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </div>
  );
}
