import type { InputHTMLAttributes } from "react";

// Shared form-field class string used throughout the settings screens.
export const INPUT_CLASS =
  "w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20";

interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  /** Overrides the default label styling (`text-xs text-secondary mb-1`). */
  labelClassName?: string;
}

export function Input({ label, labelClassName = "block text-xs text-secondary mb-1", className = "", ...rest }: InputProps) {
  if (label) {
    return (
      <div>
        <label className={labelClassName}>{label}</label>
        <input className={`${INPUT_CLASS} ${className}`} {...rest} />
      </div>
    );
  }
  return <input className={`${INPUT_CLASS} ${className}`} {...rest} />;
}
