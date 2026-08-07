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
};

/** Return the logo asset path for a provider, or undefined if unknown. */
export function getProviderLogo(providerName: string): string | undefined {
  return PROVIDER_LOGOS[providerName.toLowerCase()] ?? PROVIDER_LOGOS[providerName];
}
