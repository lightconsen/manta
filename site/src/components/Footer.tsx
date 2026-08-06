import { MessageSquare, FileText } from "lucide-react";
import GithubMark from "./GithubMark";

const base = import.meta.env.BASE_URL;

export default function Footer() {
  return (
    <footer className="border-t border-line">
      <div className="mx-auto max-w-6xl px-6 py-14">
        <div className="grid gap-10 md:grid-cols-3">
          <div>
            <div className="flex items-center gap-2.5">
              <img
                src={`${base}syscity.png`}
                alt="Syscity logo"
                className="h-7 w-7 rounded-md object-contain"
              />
              <span className="text-sm font-semibold">Syscity</span>
            </div>
            <p className="mt-3 max-w-xs text-sm text-muted">
              AI agents that control your computer. Local-first, one runtime,
              every device.
            </p>
          </div>

          <div>
            <p className="mb-3 text-xs font-semibold uppercase tracking-wider text-faint">
              Product
            </p>
            <ul className="space-y-2 text-sm text-muted">
              <li><a href="#features" className="transition hover:text-ink">Features</a></li>
              <li><a href="#platforms" className="transition hover:text-ink">Platforms</a></li>
              <li><a href="#quickstart" className="transition hover:text-ink">Quick Start</a></li>
            </ul>
          </div>

          <div>
            <p className="mb-3 text-xs font-semibold uppercase tracking-wider text-faint">
              Community
            </p>
            <ul className="space-y-2 text-sm text-muted">
              <li>
                <a
                  href="https://github.com/lightconsen/syscity"
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex items-center gap-2 transition hover:text-ink"
                >
                  <GithubMark className="h-4 w-4" /> GitHub
                </a>
              </li>
              <li>
                <a
                  href="https://discord.gg/aaXghvzD"
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex items-center gap-2 transition hover:text-ink"
                >
                  <MessageSquare className="h-4 w-4" /> Discord
                </a>
              </li>
              <li>
                <a
                  href="https://github.com/lightconsen/syscity/tree/main/docs"
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex items-center gap-2 transition hover:text-ink"
                >
                  <FileText className="h-4 w-4" /> Documentation
                </a>
              </li>
            </ul>
          </div>
        </div>

        <div className="mt-12 flex flex-col items-center justify-between gap-3 border-t border-line pt-6 text-xs text-faint sm:flex-row">
          <p>© {new Date().getFullYear()} Syscity</p>
          <p>Apache-2.0 Licensed · Open Source</p>
        </div>
      </div>
    </footer>
  );
}
