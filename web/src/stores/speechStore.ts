import { create } from "zustand";

/** Speech language choices. "system" follows the OS/browser language. */
export type SpeechLangChoice = "system" | "zh-CN" | "zh-TW" | "en-US" | "ja-JP" | "ko-KR";

const CHOICES: SpeechLangChoice[] = ["system", "zh-CN", "zh-TW", "en-US", "ja-JP", "ko-KR"];

interface SpeechState {
  lang: SpeechLangChoice;
  setLang: (lang: SpeechLangChoice) => void;
}

/** Resolve the effective BCP-47 tag for a stored choice. */
export function resolveSpeechLang(choice: SpeechLangChoice): string {
  if (choice !== "system") return choice;
  return (typeof navigator !== "undefined" && navigator.language) || "zh-CN";
}

const stored = localStorage.getItem("syscity_speech_lang") as SpeechLangChoice | null;
const initialLang: SpeechLangChoice = stored && CHOICES.includes(stored) ? stored : "system";

export const useSpeechStore = create<SpeechState>((set) => ({
  lang: initialLang,

  setLang: (lang) => {
    localStorage.setItem("syscity_speech_lang", lang);
    set({ lang });
  },
}));
