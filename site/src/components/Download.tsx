import { useLanguage } from "../i18n";

const base = import.meta.env.BASE_URL;

const DESKTOP = [
  { name: "macOS", icon: `${base}assets/platforms/mac.svg`, url: "https://syscity.net/syscity-macos.dmg" },
  { name: "Windows", icon: `${base}assets/platforms/window.svg`, url: "https://syscity.net/syscity-windows.exe" },
  { name: "Linux", icon: `${base}assets/platforms/linux.svg`, url: "https://syscity.net/syscity-linux.AppImage" },
];

const MOBILE = [
  { name: "iOS", icon: `${base}assets/platforms/ios.svg`, url: "https://apps.apple.com/app/syscity" },
  { name: "Android", icon: `${base}assets/platforms/android.svg`, url: "https://play.google.com/store/apps/details?id=net.syscity" },
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
          <p className="mt-4 text-xl font-bold text-brand-600">
            {t.download.subtitle}
          </p>
          <p className="mx-auto mt-4 max-w-2xl text-muted">{t.download.lead}</p>
        </div>

        {/* Desktop downloads */}
        <div className="mb-8 flex justify-center gap-3">
          {DESKTOP.map((p) => (
            <a
              key={p.name}
              href={p.url}
              target="_blank"
              rel="noreferrer"
              title={p.name}
              className="card inline-flex items-center justify-center rounded-xl p-3 transition hover:border-brand-500/40 hover:shadow-[0_8px_24px_rgba(178,42,194,0.10)]"
            >
              <img src={p.icon} alt={p.name} className="h-12 w-12" loading="lazy" />
            </a>
          ))}
        </div>

        {/* Mobile app stores */}
        <div className="mb-8 flex justify-center gap-3">
          {MOBILE.map((p) => (
            <a
              key={p.name}
              href={p.url}
              target="_blank"
              rel="noreferrer"
              title={p.name}
              className="card inline-flex items-center justify-center rounded-xl p-3 transition hover:border-brand-500/40 hover:shadow-[0_8px_24px_rgba(178,42,194,0.10)]"
            >
              <img src={p.icon} alt={p.name} className="h-12 w-12" loading="lazy" />
            </a>
          ))}
        </div>

        <p className="text-center text-sm text-muted">{t.download.cloudNote}</p>
      </div>
    </section>
  );
}
