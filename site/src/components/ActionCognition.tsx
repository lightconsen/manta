import {
  MousePointer,
  Apple,
  Terminal,
  Code2,
  Globe,
  FolderOpen,
  Brain,
  GitBranch,
  Database,
  Plug,
  Blocks,
} from "lucide-react";
import type { ReactNode } from "react";

const ACTION: { icon: ReactNode; label: string }[] = [
  { icon: <MousePointer className="h-4 w-4" />, label: "Desktop Control" },
  { icon: <Apple className="h-4 w-4" />, label: "AppleScript" },
  { icon: <Terminal className="h-4 w-4" />, label: "Shell Commands" },
  { icon: <Code2 className="h-4 w-4" />, label: "Code Execution" },
  { icon: <Globe className="h-4 w-4" />, label: "Browser Automation" },
  { icon: <FolderOpen className="h-4 w-4" />, label: "File Operations" },
];

const COGNITION: { icon: ReactNode; label: string }[] = [
  { icon: <Brain className="h-4 w-4" />, label: "Multi-Provider LLM" },
  { icon: <GitBranch className="h-4 w-4" />, label: "Sub-Agents (ACP)" },
  { icon: <Database className="h-4 w-4" />, label: "Vector Memory" },
  { icon: <Plug className="h-4 w-4" />, label: "MCP Support" },
  { icon: <Blocks className="h-4 w-4" />, label: "WASM Plugins" },
];

function Column({
  eyebrow,
  title,
  items,
}: {
  eyebrow: string;
  title: string;
  items: { icon: ReactNode; label: string }[];
}) {
  return (
    <div className="card rounded-2xl p-7">
      <p className="text-xs font-semibold uppercase tracking-[0.18em] text-brand-400">{eyebrow}</p>
      <h3 className="mt-1.5 text-xl font-bold">{title}</h3>
      <ul className="mt-6 space-y-3.5">
        {items.map((i) => (
          <li key={i.label} className="flex items-center gap-3 text-sm text-ink/90">
            <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-panel-2 text-brand-400 ring-1 ring-line">
              {i.icon}
            </span>
            {i.label}
          </li>
        ))}
      </ul>
    </div>
  );
}

export default function ActionCognition() {
  return (
    <section className="mx-auto max-w-6xl px-6 py-24">
      <div className="mb-12 text-center">
        <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
          Action. <span className="text-gradient">Cognition.</span>
        </h2>
        <p className="mx-auto mt-4 max-w-2xl text-muted">
          An agent system bridges language models with real computing
          environments — an action layer, a memory layer, and a control plane.
        </p>
      </div>

      <div className="grid gap-6 md:grid-cols-2">
        <Column
          eyebrow="Action"
          title="Things it does on your machine"
          items={ACTION}
        />
        <Column
          eyebrow="Cognition"
          title="How it thinks and remembers"
          items={COGNITION}
        />
      </div>
    </section>
  );
}
