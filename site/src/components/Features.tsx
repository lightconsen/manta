import {
  MousePointer,
  Globe,
  Plug,
  Lock,
  Layers,
  Cpu,
} from "lucide-react";
import type { ReactNode } from "react";

const FEATURES: { icon: ReactNode; title: string; body: string }[] = [
  {
    icon: <MousePointer className="h-5 w-5" />,
    title: "Your desktop is the canvas",
    body: "Click buttons, type text, read UI trees, take screenshots. Agents act on your computer — not just chat.",
  },
  {
    icon: <Globe className="h-5 w-5" />,
    title: "Your browser, automated",
    body: "Navigate, fill forms, capture network requests, debug console errors with sourcemaps. The agent debugs like a developer.",
  },
  {
    icon: <Plug className="h-5 w-5" />,
    title: "Your tools, connected",
    body: "MCP servers, shell commands, file operations, AppleScript. Bring your own ecosystem into the loop.",
  },
  {
    icon: <Lock className="h-5 w-5" />,
    title: "Your data, private",
    body: "Runs 100% locally. Vector memory, knowledge bases, and artifacts stay on your machine.",
  },
  {
    icon: <Layers className="h-5 w-5" />,
    title: "Every platform, one agent",
    body: "macOS, Windows, Linux, iOS, Android. The same runtime and memory, on every device you own.",
  },
  {
    icon: <Cpu className="h-5 w-5" />,
    title: "Multiple models, one agent",
    body: "Swap between OpenAI, Anthropic, DeepSeek, GLM, Ollama, or custom endpoints. Use the right model for each task.",
  },
];

export default function Features() {
  return (
    <section id="features" className="border-y border-line bg-alt">
      <div className="mx-auto max-w-6xl px-6 py-28">
        <div className="mb-14 max-w-2xl">
          <p className="mb-3 text-xs font-semibold uppercase tracking-[0.18em] text-brand-500">
            Features
          </p>
          <h2 className="text-3xl font-black tracking-tight sm:text-4xl">
            Why <span className="text-gradient">Syscity</span>?
          </h2>
          <p className="mt-4 text-muted">
            Most “AI agents” are just chatbots with function calling. Syscity
            agents control your computer — not just your API keys.
          </p>
        </div>

        <div className="grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
          {FEATURES.map((f) => (
            <article
              key={f.title}
              className="card rounded-xl p-6 transition hover:border-brand-500/40 hover:shadow-[0_12px_32px_rgba(178,42,194,0.10)]"
            >
              <div className="mb-4 inline-flex h-10 w-10 items-center justify-center rounded-lg bg-brand-500/10 text-brand-500">
                {f.icon}
              </div>
              <h3 className="mb-2 text-base font-bold">{f.title}</h3>
              <p className="text-sm leading-relaxed text-muted">{f.body}</p>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
