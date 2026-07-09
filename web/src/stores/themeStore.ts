import { create } from "zustand";

interface ThemeState {
  theme: "light" | "dark" | "system";
  resolvedTheme: "light" | "dark";

  setTheme: (theme: "light" | "dark" | "system") => void;
}

function resolveTheme(theme: "light" | "dark" | "system"): "light" | "dark" {
  if (theme !== "system") return theme;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

const stored = localStorage.getItem("syscity_theme") as "light" | "dark" | "system" | null;
const initialTheme: "light" | "dark" | "system" =
  stored === "light" || stored === "dark" || stored === "system" ? stored : "system";

function applyThemeClass(theme: "light" | "dark" | "system") {
  if (typeof document === "undefined") return;
  if (resolveTheme(theme) === "dark") {
    document.documentElement.classList.add("dark");
  } else {
    document.documentElement.classList.remove("dark");
  }
}

applyThemeClass(initialTheme);

export const useThemeStore = create<ThemeState>((set) => ({
  theme: initialTheme,
  resolvedTheme: resolveTheme(initialTheme),

  setTheme: (theme) => {
    localStorage.setItem("syscity_theme", theme);
    applyThemeClass(theme);
    set({ theme, resolvedTheme: resolveTheme(theme) });
  },
}));
