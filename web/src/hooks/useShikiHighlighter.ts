import { useEffect, useState } from "react";
import { createHighlighter, type Highlighter } from "shiki";

let highlighterPromise: Promise<Highlighter> | null = null;

function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: ["github-light", "github-dark"],
      langs: [
        "typescript",
        "javascript",
        "python",
        "rust",
        "bash",
        "json",
        "yaml",
        "markdown",
        "html",
        "css",
        "sql",
        "go",
        "tsx",
        "jsx",
        "xml",
        "dockerfile",
        "toml",
      ],
    });
  }
  return highlighterPromise;
}

export function useShikiHighlighter() {
  const [highlighter, setHighlighter] = useState<Highlighter | null>(null);

  useEffect(() => {
    let cancelled = false;
    getHighlighter().then((h) => {
      if (!cancelled) setHighlighter(h);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return highlighter;
}
