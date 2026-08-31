import { Apple, Bot, Monitor, Terminal, Smartphone } from "lucide-react";
import { useLanguage } from "../i18n";

const DESKTOP = [
  { name: "macOS", icon: Apple, url: "https://syscity.net/syscity-macos.dmg" },
  { name: "Windows", icon: Monitor, url: "https://syscity.net/syscity-windows.exe" },
  { name: "Linux", icon: Terminal, url: "https://syscity.net/syscity-linux.AppImage" },
];

const MOBILE = [
  { name: "iOS", icon: Smartphone, url: "https://apps.apple.com/app/syscity" },
  { name: "Android", icon: Bot, url: "https://play.google.com/store/apps/details?id=net.syscity" },
];

/** Download section: the merged Platforms + Download area — per-platform
 * clients, with desktop builds as direct downloads and mobile linking to the
 * app stores. */
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

        {/* Desktop downloads */}
        <div className="mb-8 grid gap-4 sm:grid-cols-3">
          {DESKTOP.map((p) => (
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

        {/* Mobile app stores */}
        <div className="mb-8 grid gap-4 sm:grid-cols-2">
          {MOBILE.map((p) => (
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

        <p className="text-center text-sm text-muted">{t.download.cloudNote}</p>
      </div>
    </section>
  );
}
