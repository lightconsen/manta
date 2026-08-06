import { Terminal } from "lucide-react";

const LINES: { prompt?: string; text: string; dim?: boolean }[] = [
  { prompt: "$", text: "curl -sSL https://syscity.net/install.sh | bash" },
  { prompt: "✓", text: "installed syscity v0.2.0", dim: true },
  { prompt: "$", text: "syscity setup" },
  { prompt: "✓", text: "configured providers.openai.api_key", dim: true },
  { prompt: "$", text: "syscity start" },
  { prompt: "🚀", text: "gateway running at http://127.0.0.1:18080", dim: true },
];

export default function QuickStart() {
  return (
    <section id="quickstart" className="border-t border-line bg-panel/40">
      <div className="mx-auto max-w-6xl px-6 py-24">
        <div className="grid items-center gap-12 lg:grid-cols-2">
          <div>
            <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
              Up and running in <span className="text-gradient">seconds</span>
            </h2>
            <p className="mt-4 max-w-md text-muted">
              No new IDE, no cloud subscription, no complex deployment. Install,
              configure, start — then ask your agent to take a screenshot,
              build a report, or automate a task.
            </p>
            <div className="mt-7 flex flex-wrap gap-3">
              <a
                href="https://github.com/lightconsen/syscity#readme"
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-2 rounded-lg bg-gradient-to-r from-brand-500 to-primary-500 px-5 py-2.5 text-sm font-semibold text-white transition hover:brightness-110"
              >
                Read the docs
              </a>
              <a
                href="https://github.com/lightconsen/syscity#quick-start"
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-2 rounded-lg border border-line bg-panel px-5 py-2.5 text-sm font-semibold transition hover:border-brand-500/50 hover:text-ink"
              >
                GitHub README
              </a>
            </div>
          </div>

          <div className="card overflow-hidden rounded-2xl">
            <div className="flex items-center gap-2 border-b border-line bg-panel-2 px-4 py-3">
              <span className="h-3 w-3 rounded-full bg-[#ff5f57]" aria-hidden="true" />
              <span className="h-3 w-3 rounded-full bg-[#febc2e]" aria-hidden="true" />
              <span className="h-3 w-3 rounded-full bg-[#28c840]" aria-hidden="true" />
              <span className="ml-3 inline-flex items-center gap-1.5 text-xs text-faint">
                <Terminal className="h-3.5 w-3.5" />
                syscity
              </span>
            </div>
            <div className="space-y-2.5 px-5 py-5 font-mono text-[13px] leading-relaxed">
              {LINES.map((l, i) => (
                <div
                  key={i}
                  className={`flex items-baseline gap-2 ${l.dim ? "text-muted" : "text-ink/90"}`}
                >
                  <span className={l.prompt === "$" ? "text-brand-400" : "text-primary-400"}>
                    {l.prompt}
                  </span>
                  <span className="break-all">{l.text}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
