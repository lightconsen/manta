import {
  MousePointer,
  Apple,
  Terminal,
  Code2,
  Globe,
  FolderOpen,
  Brain,
  GitBranch,
  Database,
  Plug,
  Blocks,
} from "lucide-react";
import type { ReactNode } from "react";
import { useLanguage } from "../i18n";

const ACTION_ICONS: ReactNode[] = [
  <MousePointer key="0" className="h-4 w-4" />,
  <Apple key="1" className="h-4 w-4" />,
  <Terminal key="2" className="h-4 w-4" />,
  <Code2 key="3" className="h-4 w-4" />,
  <Globe key="4" className="h-4 w-4" />,
  <FolderOpen key="5" className="h-4 w-4" />,
];

const COGNITION_ICONS: ReactNode[] = [
  <Brain key="0" className="h-4 w-4" />,
  <GitBranch key="1" className="h-4 w-4" />,
  <Database key="2" className="h-4 w-4" />,
  <Plug key="3" className="h-4 w-4" />,
  <Blocks key="4" className="h-4 w-4" />,
];

function Column({
  eyebrow,
  title,
  items,
}: {
  eyebrow: string;
  title: string;
  items: { icon: ReactNode; label: string }[];
}) {
  return (
    <div className="card rounded-xl p-7">
      <p className="text-xs font-semibold uppercase tracking-[0.18em] text-brand-500">{eyebrow}</p>
      <h3 className="mt-1.5 text-xl font-bold">{title}</h3>
      <ul className="mt-6 space-y-3.5">
        {items.map((i) => (
          <li key={i.label} className="flex items-center gap-3 text-sm text-ink/90">
            <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-brand-500/10 text-brand-500">
              {i.icon}
            </span>
            {i.label}
          </li>
        ))}
      </ul>
    </div>
  );
}

export default function ActionCognition() {
  const { t } = useLanguage();

  return (
    <section className="mx-auto max-w-6xl px-6 py-28">
      <div className="mb-14 text-center">
        <h2 className="text-3xl font-black tracking-tight sm:text-4xl">
          {t.actionCognition.titleA}{" "}
          <span className="text-gradient">{t.actionCognition.titleB}</span>
        </h2>
        <p className="mx-auto mt-4 max-w-2xl text-muted">{t.actionCognition.lead}</p>
      </div>

      <div className="grid gap-6 md:grid-cols-2">
        <Column
          eyebrow={t.actionCognition.actionEyebrow}
          title={t.actionCognition.actionTitle}
          items={t.actionCognition.actionItems.map((label, i) => ({
            icon: ACTION_ICONS[i],
            label,
          }))}
        />
        <Column
          eyebrow={t.actionCognition.cognitionEyebrow}
          title={t.actionCognition.cognitionTitle}
          items={t.actionCognition.cognitionItems.map((label, i) => ({
            icon: COGNITION_ICONS[i],
            label,
          }))}
        />
      </div>
    </section>
  );
}
