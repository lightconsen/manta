import { useThemeStore } from "@/stores/themeStore";
import { getProviderLogoSrc } from "@/lib/providerLogos";

interface ProviderLogoProps {
  /** Config key of the provider (e.g. "openai", "kimi"). */
  provider: string;
  /** Display name, used for the first-letter fallback when no logo exists. */
  name?: string;
  className?: string;
}

/**
 * Theme-aware provider logo. Monochrome marks swap to a white/dark SVG variant
 * so they stay legible on both themes; unknown providers fall back to a
 * first-letter chip.
 */
export function ProviderLogo({ provider, name, className = "w-5 h-5" }: ProviderLogoProps) {
  const theme = useThemeStore((s) => s.resolvedTheme);
  const src = getProviderLogoSrc(provider, theme);
  if (src) {
    return <img src={src} alt="" className={`${className} shrink-0 object-contain`} />;
  }
  return (
    <span
      className={`${className} shrink-0 rounded bg-sidebar flex items-center justify-center text-[10px] font-semibold text-secondary`}
    >
      {(name || provider).charAt(0).toUpperCase()}
    </span>
  );
}
