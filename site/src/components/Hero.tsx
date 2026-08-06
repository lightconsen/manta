import { useState } from "react";
import { Check, Copy, ArrowRight } from "lucide-react";
import GithubMark from "./GithubMark";

const base = import.meta.env.BASE_URL;
const INSTALL_CMD = "curl -sSL https://syscity.net/install.sh | bash";

function InstallChip() {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(INSTALL_CMD);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      /* clipboard unavailable — ignore */
    }
  };

  return (
    <div className="mx-auto flex max-w-xl items-center gap-3 rounded-xl border border-line bg-panel px-4 py-3 font-mono text-[13px] text-muted">
      <span className="text-brand-400">$</span>
      <span className="flex-1 truncate text-left text-ink/90">{INSTALL_CMD}</span>
      <button
        onClick={copy}
        aria-label="Copy install command"
        className="shrink-0 rounded-md p-1.5 text-muted transition hover:bg-line hover:text-ink"
      >
        {copied ? <Check className="h-4 w-4 text-brand-400" /> : <Copy className="h-4 w-4" />}
      </button>
    </div>
  );
}

export default function Hero() {
  return (
    <section id="top" className="relative overflow-hidden">
      <div className="hero-glow pointer-events-none absolute inset-0" aria-hidden="true" />

      <div className="relative mx-auto flex max-w-4xl flex-col items-center px-6 pb-24 pt-20 text-center sm:pt-28">
        <div className="animate-fade-up mb-7">
          <div className="animate-float">
            <img
              src={`${base}syscity.png`}
              alt="Syscity logo"
              className="h-24 w-24 rounded-2xl object-contain drop-shadow-[0_0_28px_rgba(178,42,194,0.45)]"
            />
          </div>
        </div>

        <p className="animate-fade-up mb-4 inline-flex items-center gap-2 rounded-full border border-line bg-panel px-3.5 py-1 text-xs font-medium uppercase tracking-[0.18em] text-brand-300 [animation-delay:80ms]">
          Syscity · AI Agent System
        </p>

        <h1 className="animate-fade-up text-4xl font-bold leading-[1.1] tracking-tight sm:text-6xl [animation-delay:160ms]">
          One agent runtime,
          <br />
          <span className="text-gradient">every device.</span>
        </h1>

        <p className="animate-fade-up mt-6 max-w-2xl text-base leading-relaxed text-muted sm:text-lg [animation-delay:240ms]">
          Syscity turns a language model into an agent that lives inside your
          machine — clicking buttons, browsing the web, running code, and
          managing files. Runs natively on macOS, Windows, Linux, iOS, and
          Android. Your data never leaves.
        </p>

        <div className="animate-fade-up mt-9 flex flex-wrap items-center justify-center gap-3 [animation-delay:320ms]">
          <a
            href="#quickstart"
            className="inline-flex items-center gap-2 rounded-lg bg-gradient-to-r from-brand-500 to-primary-500 px-5 py-2.5 text-sm font-semibold text-white shadow-[0_0_24px_rgba(178,42,194,0.35)] transition hover:brightness-110"
          >
            Get Started
            <ArrowRight className="h-4 w-4" />
          </a>
          <a
            href="https://github.com/lightconsen/syscity"
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-2 rounded-lg border border-line bg-panel px-5 py-2.5 text-sm font-semibold transition hover:border-brand-500/50 hover:text-ink"
          >
            <GithubMark className="h-4 w-4" />
            View on GitHub
          </a>
        </div>

        <div className="animate-fade-up mt-10 w-full [animation-delay:400ms]">
          <InstallChip />
        </div>
      </div>
    </section>
  );
}
