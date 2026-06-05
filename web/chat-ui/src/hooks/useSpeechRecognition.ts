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
  lang = "zh-CN",
}: UseSpeechRecognitionOptions) {
  const [isListening, setIsListening] = useState(false);
  const [supported, setSupported] = useState(false);
  const recognitionRef = useRef<SpeechRecognition | null>(null);
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
    recognition.continuous = true;
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
      // "aborted" and "no-speech" are expected during normal stop / silence
      if (event.error !== "aborted" && event.error !== "no-speech") {
        onError?.(event.error);
      }
    };

    recognition.onend = () => {
      setIsListening(false);
    };

    recognitionRef.current = recognition;

    return () => {
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
    try {
      recognitionRef.current.start();
    } catch {
      // Already started — stop and restart
      try {
        recognitionRef.current.stop();
      } catch {
        // ignore
      }
      setTimeout(() => {
        try {
          recognitionRef.current?.start();
        } catch (e) {
          onError?.(e instanceof Error ? e.message : String(e));
        }
      }, 100);
    }
  }, [onError]);

  const stop = useCallback(() => {
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
