import { useEffect, useState } from "react";

interface LiveStatusBarProps {
  liveStatus: { status: "thinking" | "tool_calling"; toolName?: string };
  startTime: number;
}

export function LiveStatusBar({ liveStatus, startTime }: LiveStatusBarProps) {
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    const id = setInterval(() => setElapsed(Date.now() - startTime), 500);
    return () => clearInterval(id);
  }, [startTime]);

  const label =
    liveStatus.status === "tool_calling"
      ? `Running ${liveStatus.toolName || "tool"}...`
      : "Thinking...";

  return (
    <div className="mt-2 flex items-center gap-2 text-xs text-gray-400 dark:text-neutral-500 animate-pulse">
      <div className="w-1.5 h-1.5 rounded-full bg-primary-500" />
      <span>{label}</span>
      <span className="font-mono">({(elapsed / 1000).toFixed(1)}s)</span>
    </div>
  );
}
