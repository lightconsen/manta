import { ArrowRight, Cloud, RefreshCw, Store } from "lucide-react";
import { useLanguage } from "../i18n";

/** "Syscity Cloud" product section (after Platforms): cloud LLM/search, device
 * sync, and the connector/expert marketplace, with a CTA to the console. */
export default function CloudSection() {
  const { t } = useLanguage();

  return (
    <section id="cloud" className="border-y border-line bg-page">
      <div className="mx-auto max-w-6xl px-6 py-28">
        <div className="mb-14 max-w-2xl">
          <p className="mb-3 text-xs font-semibold uppercase tracking-[0.18em] text-brand-500">
            {t.cloud.eyebrow}
          </p>
          <h2 className="text-3xl font-black tracking-tight sm:text-4xl">
            {t.cloud.titleA}{" "}
            <span className="text-gradient">{t.cloud.titleB}</span>
          </h2>
          <p className="mt-4 text-muted">{t.cloud.lead}</p>
        </div>

        <div className="grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
          <article className="card rounded-xl p-6 transition hover:border-brand-500/40">
            <div className="mb-4 inline-flex h-10 w-10 items-center justify-center rounded-lg bg-brand-500/10 text-brand-500">
              <Cloud className="h-5 w-5" />
            </div>
            <h3 className="text-base font-bold">{t.cloud.card1Title}</h3>
            <p className="mt-1.5 text-sm text-muted">{t.cloud.card1Desc}</p>
          </article>
          <article className="card rounded-xl p-6 transition hover:border-brand-500/40">
            <div className="mb-4 inline-flex h-10 w-10 items-center justify-center rounded-lg bg-brand-500/10 text-brand-500">
              <RefreshCw className="h-5 w-5" />
            </div>
            <h3 className="text-base font-bold">{t.cloud.card2Title}</h3>
            <p className="mt-1.5 text-sm text-muted">{t.cloud.card2Desc}</p>
          </article>
          <article className="card rounded-xl p-6 transition hover:border-brand-500/40">
            <div className="mb-4 inline-flex h-10 w-10 items-center justify-center rounded-lg bg-brand-500/10 text-brand-500">
              <Store className="h-5 w-5" />
            </div>
            <h3 className="text-base font-bold">{t.cloud.card3Title}</h3>
            <p className="mt-1.5 text-sm text-muted">{t.cloud.card3Desc}</p>
          </article>
        </div>

        <div className="mt-12 text-center">
          <a
            href="https://cloud.syscity.net"
            target="_blank"
            rel="noreferrer"
            className="inline-flex h-12 items-center gap-2 rounded-md bg-brand-500 px-6 text-[15px] font-semibold text-white shadow-[0_8px_24px_rgba(178,42,194,0.3)] transition hover:bg-brand-600"
          >
            {t.cloud.cta}
            <ArrowRight className="h-4 w-4" />
          </a>
        </div>
      </div>
    </section>
  );
}
