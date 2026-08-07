import type { ReactNode } from "react";

interface SectionProps {
  title: string;
  /** Optional header action rendered on the right of the title (e.g. "+ Add"). */
  right?: ReactNode;
  children: ReactNode;
}

export function Section({ title, right, children }: SectionProps) {
  if (right) {
    return (
      <section>
        <div className="flex items-center justify-between mb-2">
          <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider">{title}</h3>
          {right}
        </div>
        {children}
      </section>
    );
  }
  return (
    <section>
      <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">{title}</h3>
      {children}
    </section>
  );
}
