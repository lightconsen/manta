import { useState, useEffect, useRef } from "react";

export function ReasoningPart({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(true);
  const [displayedText, setDisplayedText] = useState("");
  const [done, setDone] = useState(false);
  const targetRef = useRef("");
  const animatingRef = useRef(false);
  const doneTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const isWaiting = text.length === 0;

  // Detect completion: text stops growing for 1.5s
  useEffect(() => {
    targetRef.current = text;

    if (isWaiting) {
      setDisplayedText("");
      setDone(false);
      return;
    }

    // Cancel previous done timer
    if (doneTimerRef.current) {
      clearTimeout(doneTimerRef.current);
    }

    // If we've caught up to target, start done timer
    if (displayedText.length >= text.length) {
      doneTimerRef.current = setTimeout(() => {
        setDone(true);
      }, 1500);
    }

    return () => {
      if (doneTimerRef.current) clearTimeout(doneTimerRef.current);
    };
  }, [text, displayedText.length, isWaiting]);

  // Smooth typing animation
  useEffect(() => {
    if (isWaiting || done) return;
    if (displayedText.length >= targetRef.current.length) return;
    if (animatingRef.current) return;

    animatingRef.current = true;
    let frameId: number;

    const animate = () => {
      setDisplayedText((prev) => {
        const target = targetRef.current;
        if (prev.length < target.length) {
          const gap = target.length - prev.length;
          const chunkSize = gap > 30 ? 4 : gap > 10 ? 2 : 1;
          frameId = requestAnimationFrame(animate);
          return target.slice(0, prev.length + chunkSize);
        }
        animatingRef.current = false;
        return prev;
      });
    };

    frameId = requestAnimationFrame(animate);
    return () => {
      cancelAnimationFrame(frameId);
      animatingRef.current = false;
    };
  }, [text, displayedText, isWaiting, done]);

  // ── Waiting state ───────────────────────────────────────────────
  if (isWaiting) {
    return (
      <div className="flex items-center gap-2 my-1 text-xs text-gray-400 dark:text-neutral-500">
        <span className="relative flex h-3 w-3">
          <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-gray-300 dark:bg-neutral-600 opacity-75" />
          <span className="relative inline-flex rounded-full h-3 w-3 bg-gray-400 dark:bg-neutral-500" />
        </span>
        <span>Thinking</span>
      </div>
    );
  }

  // ── Done (collapsed) ────────────────────────────────────────────
  if (done && !expanded) {
    return (
      <button
        onClick={() => setExpanded(true)}
        className="flex items-center gap-1.5 my-1 text-[11px] text-gray-400 dark:text-neutral-500 hover:text-gray-600 dark:hover:text-neutral-400 transition"
      >
        <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.548.547A3.374 3.374 0 0014 18.469V19a2 2 0 11-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z" />
        </svg>
        <span>Thought process</span>
        <span className="text-gray-300 dark:text-neutral-600">({text.length} chars)</span>
      </button>
    );
  }

  // ── Typing or expanded done ─────────────────────────────────────
  const isTyping = displayedText.length < text.length && !done;

  return (
    <div className="my-1.5 rounded-md border border-gray-200 dark:border-neutral-700 bg-gray-50/50 dark:bg-neutral-800/30 overflow-hidden">
      {/* Header */}
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center gap-1.5 px-2.5 py-1.5 text-[11px] text-gray-500 dark:text-neutral-400 hover:bg-gray-100/50 dark:hover:bg-neutral-700/30 transition"
      >
        <svg
          className={`w-3 h-3 transition-transform ${expanded ? "rotate-90" : ""}`}
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
        </svg>
        <span>Thought process</span>
        {isTyping && (
          <span className="ml-auto inline-flex gap-0.5">
            <span className="w-1 h-1 rounded-full bg-gray-400 dark:bg-neutral-500 animate-bounce [animation-delay:0ms]" />
            <span className="w-1 h-1 rounded-full bg-gray-400 dark:bg-neutral-500 animate-bounce [animation-delay:120ms]" />
            <span className="w-1 h-1 rounded-full bg-gray-400 dark:bg-neutral-500 animate-bounce [animation-delay:240ms]" />
          </span>
        )}
        {!isTyping && done && (
          <span className="ml-auto text-gray-300 dark:text-neutral-600">done</span>
        )}
      </button>

      {/* Content */}
      {expanded && (
        <div className="px-3 py-2 text-[11px] text-gray-600 dark:text-neutral-400 font-mono whitespace-pre-wrap leading-relaxed border-t border-gray-200 dark:border-neutral-700 max-h-64 overflow-y-auto">
          {displayedText}
          {isTyping && (
            <span className="inline-block w-1.5 h-3.5 ml-0.5 align-text-bottom bg-gray-400 dark:bg-neutral-500 animate-pulse" />
          )}
        </div>
      )}
    </div>
  );
}
