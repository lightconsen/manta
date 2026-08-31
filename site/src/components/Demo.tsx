import { useLanguage } from "../i18n";

const base = import.meta.env.BASE_URL;

export default function Demo() {
  const { t } = useLanguage();

  return (
    <section className="mx-auto max-w-5xl px-6 pb-24">
      <div className="card overflow-hidden rounded-2xl shadow-[0_24px_64px_rgba(25,26,35,0.10)]">
        <div className="flex items-center gap-2 border-b border-line bg-panel-2 px-4 py-3">
          <span className="h-3 w-3 rounded-full bg-[#ff5f57]" aria-hidden="true" />
          <span className="h-3 w-3 rounded-full bg-[#febc2e]" aria-hidden="true" />
          <span className="h-3 w-3 rounded-full bg-[#28c840]" aria-hidden="true" />
          <span className="ml-3 text-xs text-faint">{t.demo.chromeTitle}</span>
        </div>
        <img
          src={`${base}assets/demo-light.gif`}
          alt={t.demo.alt}
          className="w-full"
          loading="lazy"
        />
      </div>
      <p className="mt-4 text-center text-sm text-muted">
        {t.demo.captionBefore} <code className="font-mono">{t.demo.captionTool}</code>
        {t.demo.captionAfter}
      </p>

      {/* Mobile app screenshots (iOS + Android), below the demo GIF. */}
      <div className="mt-10 grid gap-6 sm:grid-cols-2">
        <img
          src={`${base}assets/mobile-ios-light.png`}
          alt="Syscity on iOS"
          className="card w-full rounded-2xl object-contain"
          loading="lazy"
        />
        <img
          src={`${base}assets/mobile-android-light.png`}
          alt="Syscity on Android"
          className="card w-full rounded-2xl object-contain"
          loading="lazy"
        />
      </div>
    </section>
  );
}
