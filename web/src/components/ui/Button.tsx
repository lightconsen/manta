import type { ButtonHTMLAttributes } from "react";

type ButtonVariant = "primary-sm" | "primary-md" | "ghost";

const VARIANTS: Record<ButtonVariant, string> = {
  "primary-sm":
    "px-3 py-1 rounded-md bg-primary-500 hover:bg-primary-600 text-white text-xs font-medium transition",
  "primary-md":
    "px-4 py-1.5 rounded-md bg-primary-500 hover:bg-primary-600 disabled:opacity-50 text-white text-xs font-medium transition",
  ghost:
    "px-3 py-1.5 text-xs bg-sidebar hover:bg-black/5 dark:hover:bg-white/5 rounded-md text-secondary transition-colors",
};

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
}

export function Button({ variant = "primary-md", className = "", ...rest }: ButtonProps) {
  return <button className={`${VARIANTS[variant]} ${className}`} {...rest} />;
}
