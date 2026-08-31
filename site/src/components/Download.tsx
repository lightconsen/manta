import { Apple, Bot, Monitor, Terminal, Smartphone } from "lucide-react";
import { useLanguage } from "../i18n";

const PLATFORMS = [
  { name: "macOS", icon: Apple, url: "https://github.com/lightconsen/syscity/releases" },
  { name: "Windows", icon: Monitor, url: "https://github.com/lightconsen/syscity/releases" },
  { name: "Linux", icon: Terminal, url: "https://github.com/lightconsen/syscity/releases" },
  { name: "iOS", icon: Smartphone, url: "https://github.com/lightconsen/syscity/releases" },
  { name: "Android", icon: Bot, url: "https://github.com/lightconsen/syscity/releases" },
];

/** Download section: per-platform client downloads (with cloud features). */
export default function Download() {
  const { t } = useLanguage();

  return (
    <section id="download" className="border-y border-line bg-alt">
      <div className="mx-auto max-w-6xl px-6 py-28">
        <div className="mb-14 text-center">
          <h2 className="text-3xl font-black tracking-tight sm:text-4xl">
            {t.download.title}
          </h2>
          <p className="mx-auto mt-4 max-w-2xl text-muted">{t.download.lead}</p>
        </div>

        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
          {PLATFORMS.map((p) => (
            <a
              key={p.name}
              href={p.url}
              target="_blank"
              rel="noreferrer"
              className="card flex flex-col items-center gap-3 rounded-xl p-6 text-center transition hover:border-brand-500/40 hover:shadow-[0_12px_32px_rgba(178,42,194,0.10)]"
            >
              <p.icon className="h-8 w-8 text-brand-500" />
              <span className="text-sm font-semibold">{p.name}</span>
            </a>
          ))}
        </div>

        <p className="mt-10 text-center text-sm text-muted">
          {t.download.cloudNote}
        </p>
      </div>
    </section>
  );
}
