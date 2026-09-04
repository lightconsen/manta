import { useEffect, useState } from "react";

/** Which shell variant the Titlebar/Statusbar chrome should render. */
export type PlatformVariant =
  | "tauri-macos" // macOS webview: traffic lights overlay the titlebar
  | "tauri-desktop" // Windows/Linux desktop: native titlebar kept
  | "tauri-mobile" // iOS/Android webview
  | "web"; // plain browser

const isTauriRuntime = typeof window !== "undefined" && "__TAURI__" in window;

// Resolved once per session so remounts read it synchronously (avoids a
// plain-header flash when switching back to a mac-inset variant).
let cached: PlatformVariant | null = null;

function initial(): PlatformVariant {
  if (cached) return cached;
  // Safe default: plain header (no inset). Inside Tauri the effect below
  // refines it; the window is hidden until gateway-ready, so the switch is
  // not user-visible.
  return "web";
}

/**
 * Detect the runtime platform for shell chrome. In Tauri it asks the shell
 * (`get_platform` — compile-time OS); in a plain browser it returns "web".
 */
export function usePlatform(): PlatformVariant {
  const [variant, setVariant] = useState<PlatformVariant>(initial);

  useEffect(() => {
    if (cached) {
      setVariant(cached);
      return;
    }
    if (!isTauriRuntime) {
      cached = "web";
      return;
    }
    let alive = true;
    (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const os = await invoke<string>("get_platform");
        const resolved: PlatformVariant =
          os === "macos"
            ? "tauri-macos"
            : os === "ios" || os === "android"
              ? "tauri-mobile"
              : "tauri-desktop";
        cached = resolved;
        if (alive) setVariant(resolved);
      } catch {
        // Outside Tauri or command unavailable — stay on the plain variant.
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  return variant;
}
