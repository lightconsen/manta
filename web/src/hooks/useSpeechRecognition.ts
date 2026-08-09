import { useState, useCallback, useRef, useEffect } from "react";

interface UseSpeechRecognitionOptions {
  onResult: (text: string) => void;
  onInterim?: (text: string) => void;
  onError?: (error: string) => void;
  onSubmit?: () => void;
  autoSubmit?: boolean;
  lang?: string;
}

/** Event pushed from the Android SpeechPlugin over a Tauri Channel. */
type NativeSpeechEvent =
  | { type: "partial"; text: string }
  | { type: "final"; text: string }
  | { type: "error"; code: string }
  | { type: "state"; value: string };

/**
 * Voice input with two engines behind one hook:
 * - web: the Web Speech API (Chromium / Safari / WebView2), used whenever the
 *   webview exposes `SpeechRecognition`.
 * - native: the Tauri `speech` plugin (Android `SpeechRecognizer`), probed
 *   via `plugin:speech|is_available` when the web API is absent.
 *
 * The conversation loop (auto-restart after a session ends, auto-submit on
 * final results) lives here so both engines behave identically.
 */
export function useSpeechRecognition({
  onResult,
  onInterim,
  onError,
  onSubmit,
  autoSubmit = false,
  lang = typeof navigator !== "undefined"
    ? navigator.language || "zh-CN"
    : "zh-CN",
}: UseSpeechRecognitionOptions) {
  const [isListening, setIsListening] = useState(false);
  const [supported, setSupported] = useState(false);
  const engineRef = useRef<"web" | "native">("web");
  const recognitionRef = useRef<SpeechRecognition | null>(null);
  const shouldRestartRef = useRef(false);
  const restartTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const submitTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Detect support on mount: web API first, then the native bridge.
  useEffect(() => {
    const SpeechRecognitionCtor =
      window.SpeechRecognition || window.webkitSpeechRecognition;
    if (SpeechRecognitionCtor) {
      engineRef.current = "web";
      setSupported(true);
      return;
    }
    if ("__TAURI__" in window) {
      import("@tauri-apps/api/core")
        .then(({ invoke }) =>
          invoke<{ available?: boolean }>("plugin:speech|is_available")
        )
        .then((res) => {
          if (res?.available) {
            engineRef.current = "native";
            setSupported(true);
          }
        })
        .catch(() => {
          // Shell without the speech plugin — voice stays unsupported.
        });
    }
  }, []);

  // ── Web engine ────────────────────────────────────────────────────────────
  // Build / rebuild recognition instance when lang changes (native engine
  // needs no eager setup).
  useEffect(() => {
    if (engineRef.current !== "web") return;
    const SpeechRecognitionCtor =
      window.SpeechRecognition || window.webkitSpeechRecognition;
    if (!SpeechRecognitionCtor) return;

    const recognition = new SpeechRecognitionCtor();
    // Use continuous=false for reliable final results;
    // auto-restart in onend keeps the loop going.
    recognition.continuous = false;
    recognition.interimResults = true;
    recognition.lang = lang;
    recognition.maxAlternatives = 1;

    recognition.onstart = () => {
      setIsListening(true);
    };

    recognition.onresult = (event: SpeechRecognitionEvent) => {
      let finalTranscript = "";
      let interimTranscript = "";

      for (let i = event.resultIndex; i < event.results.length; i++) {
        const transcript = event.results[i][0].transcript;
        if (event.results[i].isFinal) {
          finalTranscript += transcript;
        } else {
          interimTranscript += transcript;
        }
      }

      if (interimTranscript) {
        onInterim?.(interimTranscript);
      }
      if (finalTranscript) {
        onResult(finalTranscript);
        if (autoSubmit && finalTranscript.trim()) {
          if (submitTimerRef.current) {
            clearTimeout(submitTimerRef.current);
          }
          submitTimerRef.current = setTimeout(() => {
            onSubmit?.();
            submitTimerRef.current = null;
          }, 300);
        }
      }
    };

    recognition.onerror = (event: SpeechRecognitionErrorEvent) => {
      // "no-speech" is normal — onend will auto-restart.
      // "aborted" happens when we call .stop() / .abort().
      if (event.error !== "aborted" && event.error !== "no-speech") {
        onError?.(event.error);
      }
    };

    recognition.onend = () => {
      setIsListening(false);
      // Auto-restart if voice mode is still active.
      if (shouldRestartRef.current) {
        restartTimerRef.current = setTimeout(() => {
          if (shouldRestartRef.current) {
            try {
              recognitionRef.current?.start();
            } catch {
              // Already started or other race — ignore.
            }
          }
        }, 300);
      }
    };

    recognitionRef.current = recognition;

    return () => {
      shouldRestartRef.current = false;
      if (restartTimerRef.current) {
        clearTimeout(restartTimerRef.current);
        restartTimerRef.current = null;
      }
      if (submitTimerRef.current) {
        clearTimeout(submitTimerRef.current);
        submitTimerRef.current = null;
      }
      try {
        recognition.abort();
      } catch {
        // ignore
      }
    };
  }, [lang, onResult, onInterim, onError, onSubmit, autoSubmit]);

  // ── Native engine (Tauri speech plugin) ───────────────────────────────────

  const startNative = useCallback(async () => {
    shouldRestartRef.current = true;
    try {
      const { invoke, Channel } = await import("@tauri-apps/api/core");

      // First run prompts for the mic; without it there is nothing to do.
      const perm = await invoke<{ granted?: boolean }>(
        "plugin:speech|request_mic_permission"
      );
      if (!perm?.granted) {
        shouldRestartRef.current = false;
        onError?.("permission");
        return;
      }

      const events = new Channel<NativeSpeechEvent>();
      events.onmessage = (evt) => {
        if (evt.type === "partial") {
          onInterim?.(evt.text);
          return;
        }
        if (evt.type === "state") {
          if (evt.value === "listening") setIsListening(true);
          return;
        }
        // "final" and "error" both end the native session (the plugin has
        // already released the recognizer).
        setIsListening(false);
        if (evt.type === "final") {
          if (evt.text) {
            onResult(evt.text);
            if (autoSubmit && evt.text.trim()) {
              if (submitTimerRef.current) {
                clearTimeout(submitTimerRef.current);
              }
              submitTimerRef.current = setTimeout(() => {
                onSubmit?.();
                submitTimerRef.current = null;
              }, 300);
            }
          }
        } else {
          // Benign endings mirror the web engine's "no-speech": silent restart.
          const benign = ["no_match", "speech_timeout", "client", "audio"];
          if (!benign.includes(evt.code)) {
            onError?.(evt.code);
          }
        }
        // Auto-restart if voice mode is still active.
        if (shouldRestartRef.current) {
          if (restartTimerRef.current) {
            clearTimeout(restartTimerRef.current);
          }
          restartTimerRef.current = setTimeout(() => {
            if (shouldRestartRef.current) {
              void startNative();
            }
          }, 300);
        }
      };

      await invoke("plugin:speech|start_listening", { lang, events });
    } catch (e) {
      onError?.(e instanceof Error ? e.message : String(e));
    }
  }, [lang, onResult, onInterim, onError, onSubmit, autoSubmit]);

  const stopNative = useCallback(async () => {
    shouldRestartRef.current = false;
    if (restartTimerRef.current) {
      clearTimeout(restartTimerRef.current);
      restartTimerRef.current = null;
    }
    setIsListening(false);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("plugin:speech|stop_listening");
    } catch {
      // Shell without the plugin — nothing to stop.
    }
  }, []);

  // Stop the native session when the component unmounts mid-listen.
  useEffect(() => {
    return () => {
      if (engineRef.current === "native") {
        shouldRestartRef.current = false;
        void stopNative();
      }
    };
  }, [stopNative]);

  // ── Shared API ────────────────────────────────────────────────────────────

  const start = useCallback(() => {
    if (engineRef.current === "native") {
      void startNative();
      return;
    }
    if (!recognitionRef.current) {
      onError?.("Browser does not support speech recognition");
      return;
    }
    shouldRestartRef.current = true;
    try {
      recognitionRef.current.start();
    } catch (e) {
      onError?.(e instanceof Error ? e.message : String(e));
    }
  }, [onError, startNative]);

  const stop = useCallback(() => {
    if (engineRef.current === "native") {
      void stopNative();
      return;
    }
    shouldRestartRef.current = false;
    if (restartTimerRef.current) {
      clearTimeout(restartTimerRef.current);
      restartTimerRef.current = null;
    }
    try {
      recognitionRef.current?.stop();
    } catch {
      // ignore
    }
    setIsListening(false);
  }, [stopNative]);

  const toggle = useCallback(() => {
    if (isListening) {
      stop();
    } else {
      start();
    }
  }, [isListening, start, stop]);

  return { isListening, start, stop, toggle, supported };
}
