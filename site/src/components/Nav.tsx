import { Star } from "lucide-react";

const base = import.meta.env.BASE_URL;

const LINKS = [
  { href: "#features", label: "Features" },
  { href: "#platforms", label: "Platforms" },
  { href: "#quickstart", label: "Quick Start" },
];

export default function Nav() {
  return (
    <header className="sticky top-0 z-50 border-b border-line bg-page/80 backdrop-blur-md">
      <nav className="mx-auto flex h-16 max-w-6xl items-center justify-between px-6">
        <a href="#top" className="flex items-center gap-2.5">
          <img
            src={`${base}syscity.png`}
            alt="Syscity logo"
            className="h-8 w-8 rounded-md object-contain"
          />
          <span className="text-[15px] font-bold tracking-tight">Syscity</span>
        </a>

        <div className="hidden items-center gap-8 text-sm font-medium text-muted md:flex">
          {LINKS.map((l) => (
            <a key={l.href} href={l.href} className="transition hover:text-ink">
              {l.label}
            </a>
          ))}
        </div>

        <div className="flex items-center gap-2.5">
          <a
            href="https://github.com/lightconsen/syscity"
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-1.5 rounded-md border border-line bg-panel px-3.5 py-2 text-sm font-semibold transition hover:border-brand-500/60 hover:text-brand-600"
          >
            <Star className="h-4 w-4" />
            <span className="hidden sm:inline">Star on GitHub</span>
            <span className="sm:hidden">Star</span>
          </a>
          <a
            href="https://discord.gg/aaXghvzD"
            target="_blank"
            rel="noreferrer"
            className="hidden rounded-md bg-brand-500 px-3.5 py-2 text-sm font-semibold text-white transition hover:bg-brand-600 sm:inline-flex"
          >
            Discord
          </a>
        </div>
      </nav>
    </header>
  );
}
