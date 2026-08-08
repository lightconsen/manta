// Shared provider logo lookup used across the model selector, model settings,
// and provider settings panels.

const PROVIDER_LOGOS: Record<string, string> = {
  openai: "/assets/providers/openai.svg",
  deepseek: "/assets/providers/deepseek.svg",
  ollama: "/assets/providers/ollama.svg",
  qwen: "/assets/providers/qwen.svg",
  kimi: "/assets/providers/moonshot.svg",
  anthropic: "/assets/providers/anthropic.svg",
  azure: "/assets/providers/azure.svg",
  gemini: "/assets/providers/gemini.svg",
  glm: "/assets/providers/chatglm.svg",
  minimax: "/assets/providers/minimax.svg",
  doubao: "/assets/providers/doubao.svg",
  hunyuan: "/assets/providers/hunyuan.svg",
  grok: "/assets/providers/grok.svg",
  mistral: "/assets/providers/mistral.svg",
  cohere: "/assets/providers/cohere.svg",
};

// Monochrome marks that need a theme variant. These SVGs render as pure black
// (`fill="currentColor"` defaults to black inside a standalone <img>) or pure
// white (`fill="#fff"`), so a single version is illegible on one of the themes.
const WHITE_VARIANT_PROVIDERS = new Set([
  "openai",
  "ollama",
  "doubao",
  "hunyuan",
  "grok",
  "mistral",
  "cohere",
]); // black mark -> white for dark
const DARK_VARIANT_PROVIDERS = new Set(["kimi"]); // white mark -> dark glyph for light

/**
 * Return the theme-appropriate logo path for a provider. Monochrome marks swap
 * to a `-white.svg` / `-dark.svg` sibling file so the logo stays legible on
 * both light and dark backgrounds.
 */
export function getProviderLogoSrc(
  providerName: string,
  theme: "light" | "dark"
): string | undefined {
  const key = providerName.toLowerCase();
  const base = PROVIDER_LOGOS[key] ?? PROVIDER_LOGOS[providerName];
  if (!base) return undefined;
  if (WHITE_VARIANT_PROVIDERS.has(key) && theme === "dark") {
    return base.replace(/\.svg$/, "-white.svg");
  }
  if (DARK_VARIANT_PROVIDERS.has(key) && theme === "light") {
    return base.replace(/\.svg$/, "-dark.svg");
  }
  return base;
}
