import { useState, useCallback, useRef, useEffect } from "react";

interface UseTextToSpeechOptions {
  onEnd?: () => void;
  lang?: string;
  rate?: number;
  pitch?: number;
}

export function useTextToSpeech({
  onEnd,
  lang = "zh-CN",
  rate = 1.0,
  pitch = 1.0,
}: UseTextToSpeechOptions) {
  const [isSpeaking, setIsSpeaking] = useState(false);
  const [supported, setSupported] = useState(false);
  const queueRef = useRef<string[]>([]);
  const speakingRef = useRef(false);
  const resumeTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    setSupported("speechSynthesis" in window);
  }, []);

  const cleanupTimer = useCallback(() => {
    if (resumeTimerRef.current) {
      clearInterval(resumeTimerRef.current);
      resumeTimerRef.current = null;
    }
  }, []);

  const speakNext = useCallback(() => {
    if (queueRef.current.length === 0) {
      speakingRef.current = false;
      setIsSpeaking(false);
      cleanupTimer();
      onEnd?.();
      return;
    }

    const text = queueRef.current.shift()!;
    const utterance = new SpeechSynthesisUtterance(text);
    utterance.lang = lang;
    utterance.rate = rate;
    utterance.pitch = pitch;

    // Chrome pauses speech after ~15s of inactivity; keep it alive
    resumeTimerRef.current = setInterval(() => {
      if (window.speechSynthesis.paused) {
        window.speechSynthesis.resume();
      }
    }, 5000);

    utterance.onend = () => {
      cleanupTimer();
      speakNext();
    };

    utterance.onerror = () => {
      cleanupTimer();
      speakNext();
    };

    window.speechSynthesis.speak(utterance);
  }, [lang, rate, pitch, onEnd, cleanupTimer]);

  const speak = useCallback(
    (text: string) => {
      if (!supported || !text.trim()) return;
      queueRef.current.push(text);
      if (!speakingRef.current) {
        speakingRef.current = true;
        setIsSpeaking(true);
        speakNext();
      }
    },
    [supported, speakNext]
  );

  const stop = useCallback(() => {
    window.speechSynthesis?.cancel();
    queueRef.current = [];
    speakingRef.current = false;
    setIsSpeaking(false);
    cleanupTimer();
  }, [cleanupTimer]);

  return { isSpeaking, speak, stop, supported };
}
