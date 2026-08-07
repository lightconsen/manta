import type { SelectHTMLAttributes } from "react";

// Shared styled <select> used throughout the settings screens.
export const SELECT_CLASS =
  "w-full rounded-lg border border-subtle bg-card px-3 py-2 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20";

interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  label?: string;
  /** Overrides the default label styling (`text-xs text-secondary mb-1`). */
  labelClassName?: string;
}

export function Select({ label, labelClassName = "block text-xs text-secondary mb-1", className = "", ...rest }: SelectProps) {
  return (
    <div>
      {label && <label className={labelClassName}>{label}</label>}
      <select className={`${SELECT_CLASS} ${className}`} {...rest} />
    </div>
  );
}
