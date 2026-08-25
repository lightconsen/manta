import {
  MousePointer,
  Globe,
  Plug,
  Lock,
  Layers,
  Cpu,
} from "lucide-react";
import type { ReactNode } from "react";
import { useLanguage } from "../i18n";

const ICONS: ReactNode[] = [
  <MousePointer key="0" className="h-5 w-5" />,
  <Globe key="1" className="h-5 w-5" />,
  <Plug key="2" className="h-5 w-5" />,
  <Lock key="3" className="h-5 w-5" />,
  <Layers key="4" className="h-5 w-5" />,
  <Cpu key="5" className="h-5 w-5" />,
];

export default function Features() {
  const { t } = useLanguage();

  return (
    <section id="features" className="border-y border-line bg-alt">
      <div className="mx-auto max-w-6xl px-6 py-28">
        <div className="mb-14 max-w-2xl">
          <p className="mb-3 text-xs font-semibold uppercase tracking-[0.18em] text-brand-500">
            {t.features.eyebrow}
          </p>
          <h2 className="text-3xl font-black tracking-tight sm:text-4xl">
            {t.features.titleBefore}{" "}
            <span className="text-gradient">{t.features.titleBrand}</span>
            {t.features.titleAfter}
          </h2>
          <p className="mt-4 text-muted">{t.features.lead}</p>
        </div>

        <div className="grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
          {t.features.items.map((f, i) => (
            <article
              key={f.title}
              className="card rounded-xl p-6 transition hover:border-brand-500/40 hover:shadow-[0_12px_32px_rgba(178,42,194,0.10)]"
            >
              <div className="mb-4 inline-flex h-10 w-10 items-center justify-center rounded-lg bg-brand-500/10 text-brand-500">
                {ICONS[i]}
              </div>
              <h3 className="mb-2 text-base font-bold">{f.title}</h3>
              <p className="text-sm leading-relaxed text-muted">{f.body}</p>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
