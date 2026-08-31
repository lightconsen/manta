import { Apple, Monitor, Terminal, Smartphone, Bot } from "lucide-react";
import type { ReactNode } from "react";
import { useLanguage } from "../i18n";

const base = import.meta.env.BASE_URL;

const PLATFORMS: { name: string; icon: ReactNode }[] = [
  { name: "macOS", icon: <Apple className="h-5 w-5" /> },
  { name: "Windows", icon: <Monitor className="h-5 w-5" /> },
  { name: "Linux", icon: <Terminal className="h-5 w-5" /> },
  { name: "iOS", icon: <Smartphone className="h-5 w-5" /> },
  { name: "Android", icon: <Bot className="h-5 w-5" /> },
];

function PhoneShot({ src, alt }: { src: string; alt: string }) {
  return (
    <figure className="card relative mx-auto w-full max-w-[320px] rounded-3xl p-2.5 shadow-[0_16px_48px_rgba(25,26,35,0.08)]">
      <div className="mx-auto mb-2 h-1 w-16 rounded-full bg-line" aria-hidden="true" />
      <img
        src={src}
        alt={alt}
        loading="lazy"
        className="w-full rounded-2xl object-contain"
      />
    </figure>
  );
}

export default function Platforms() {
  const { t } = useLanguage();

  return (
    <section id="platforms" className="border-y border-line bg-alt">
      <div className="mx-auto max-w-6xl px-6 py-28">
        <div className="mb-14 text-center">
          <h2 className="text-3xl font-black tracking-tight sm:text-4xl">
            {t.platforms.titleA}{" "}
            <span className="text-gradient">{t.platforms.titleB}</span>
          </h2>
          <p className="mx-auto mt-4 max-w-2xl text-muted">{t.platforms.lead}</p>
        </div>

        <div className="mb-14 flex flex-wrap items-center justify-center gap-3">
          {PLATFORMS.map((p) => (
            <span
              key={p.name}
              className="inline-flex items-center gap-2 rounded-full border border-line bg-panel px-4 py-2 text-sm font-medium text-ink/90"
            >
              <span className="text-brand-500">{p.icon}</span>
              {p.name}
            </span>
          ))}
        </div>

        <div className="grid gap-6 sm:grid-cols-2">
          <PhoneShot
            src={`${base}assets/mobile-ios-light.png`}
            alt={t.platforms.iosAlt}
          />
          <PhoneShot
            src={`${base}assets/mobile-android-light.png`}
            alt={t.platforms.androidAlt}
          />
        </div>

        <div className="mt-10 text-center">
          <a
            href="#download"
            className="inline-flex items-center gap-2 text-sm font-semibold text-brand-500 transition hover:text-brand-600"
          >
            {t.platforms.downloadCta} →
          </a>
        </div>
      </div>
    </section>
  );
}
