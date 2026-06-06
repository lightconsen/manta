import { useState, useCallback, useRef, useEffect } from "react";

interface UseSpeechRecognitionOptions {
  onResult: (text: string) => void;
  onInterim?: (text: string) => void;
  onError?: (error: string) => void;
  onSubmit?: () => void;
  autoSubmit?: boolean;
  lang?: string;
}

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
  const recognitionRef = useRef<SpeechRecognition | null>(null);
  const shouldRestartRef = useRef(false);
  const restartTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const submitTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Detect support on mount
  useEffect(() => {
    const SpeechRecognitionCtor =
      window.SpeechRecognition || window.webkitSpeechRecognition;
    setSupported(!!SpeechRecognitionCtor);
  }, []);

  // Build / rebuild recognition instance when lang changes
  useEffect(() => {
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

  const start = useCallback(() => {
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
  }, [onError]);

  const stop = useCallback(() => {
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
  }, []);

  const toggle = useCallback(() => {
    if (isListening) {
      stop();
    } else {
      start();
    }
  }, [isListening, start, stop]);

  return { isListening, start, stop, toggle, supported };
}
