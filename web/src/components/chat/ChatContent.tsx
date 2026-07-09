import { useRef, useState, useCallback, useEffect } from "react";
import {
  ThreadPrimitive,
  ComposerPrimitive,
  useComposerRuntime,
} from "@assistant-ui/react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { MessageBubble } from "./MessageBubble";
import { CommandPalette } from "./CommandPalette";
import { getCommandCompletions, type CommandDef } from "@/slash-commands";
import { useChatStore } from "@/stores/chatStore";
import { Mic, Image, Paperclip, Square, Send } from "lucide-react";
import { MessageSkeleton } from "@/components/ui/Skeleton";
import { useSpeechRecognition } from "@/hooks/useSpeechRecognition";
import { useTextToSpeech } from "@/hooks/useTextToSpeech";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";

interface ChatContentProps {
  transport: SyscityWebSocketTransport;
}

export function ChatContent({ transport }: ChatContentProps) {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const composer = useComposerRuntime();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteIndex, setPaletteIndex] = useState(0);
  const [paletteCommands, setPaletteCommands] = useState<CommandDef[]>([]);

  const messages = useChatStore((s) => s.messages);
  const isRunning = useChatStore((s) => s.isRunning);
  const voiceMode = useChatStore((s) => s.voiceMode);
  const setVoiceMode = useChatStore((s) => s.setVoiceMode);

  /* ── TTS: auto-read AI replies in voice mode ── */
  const {
    isSpeaking,
    speak,
    stop: stopSpeaking,
    supported: ttsSupported,
  } = useTextToSpeech({
    lang: "zh-CN",
    onEnd: () => {
      // After speaking, re-start listening if still in voice mode
      if (useChatStore.getState().voiceMode) {
        setTimeout(() => startListening(), 400);
      }
    },
  });

  /* ── STT: voice input with auto-submit ── */
  const handleVoiceResult = useCallback(
    (text: string) => {
      const current = inputRef.current?.value || "";
      const next = current ? `${current} ${text}` : text;
      composer.setText(next);
      setTimeout(() => inputRef.current?.focus(), 0);
    },
    [composer]
  );

  const {
    isListening,
    start: startListening,
    stop: stopListening,
    supported: sttSupported,
  } = useSpeechRecognition({
    onResult: handleVoiceResult,
    onError: (err) => console.warn("Speech recognition error:", err),
    onSubmit: () => {
      // Auto-send when voice input completes (only in voice mode)
      if (useChatStore.getState().voiceMode) {
        composer.send?.();
      }
    },
    autoSubmit: true,
    lang: "zh-CN",
  });

  const voiceSupported = sttSupported && ttsSupported;

  /* ── Voice mode toggle ── */
  const toggleVoiceMode = useCallback(() => {
    const next = !voiceMode;
    setVoiceMode(next);
    if (next) {
      startListening();
    } else {
      stopListening();
      stopSpeaking();
    }
  }, [voiceMode, setVoiceMode, startListening, stopListening, stopSpeaking]);

  /* ── Auto-read when AI reply completes ── */
  const prevIsRunningRef = useRef(isRunning);
  useEffect(() => {
    const wasRunning = prevIsRunningRef.current;
    prevIsRunningRef.current = isRunning;

    if (wasRunning && !isRunning && voiceMode) {
      const msgs = useChatStore.getState().messages;
      const last = msgs[msgs.length - 1];
      if (last?.role === "assistant" && last.content) {
        speak(last.content);
      }
    }
  }, [isRunning, voiceMode, speak]);

  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 120,
    measureElement: (el) => el.getBoundingClientRect().height,
    overscan: 5,
  });

  // Sync run state into store
  useEffect(() => {
    return transport.onRunStateChange((running) => {
      useChatStore.getState().setIsRunning(running);
    });
  }, [transport]);

  // Auto-scroll to bottom when new messages arrive
  useEffect(() => {
    if (messages.length > 0) {
      requestAnimationFrame(() => {
        virtualizer.scrollToIndex(messages.length - 1, { align: "end" });
      });
    }
  }, [messages.length, virtualizer]);

  const handleInput = useCallback(() => {
    const val = inputRef.current?.value || "";
    if (val.startsWith("/")) {
      const filter = val.slice(1).split(" ")[0] || "";
      const cmds = getCommandCompletions(filter);
      setPaletteCommands(cmds);
      setPaletteOpen(cmds.length > 0);
      setPaletteIndex(0);
    } else {
      setPaletteOpen(false);
    }
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (!paletteOpen) return;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setPaletteIndex((i) => Math.min(i + 1, paletteCommands.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setPaletteIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        const cmd = paletteCommands[paletteIndex];
        if (cmd) {
          composer.setText(`/${cmd.name} `);
          setPaletteOpen(false);
          setTimeout(() => inputRef.current?.focus(), 0);
        }
      } else if (e.key === "Escape") {
        setPaletteOpen(false);
      }
    },
    [paletteOpen, paletteCommands, paletteIndex, composer]
  );

  const handleSelectCommand = useCallback(
    (cmd: CommandDef) => {
      composer.setText(`/${cmd.name} `);
      setPaletteOpen(false);
      setTimeout(() => inputRef.current?.focus(), 0);
    },
    [composer]
  );

  const virtualItems = virtualizer.getVirtualItems();
  const totalHeight = virtualizer.getTotalSize();

  /* ── Mic button helpers ── */
  const micTitle = !voiceSupported
    ? "Voice not supported"
    : voiceMode
      ? isListening
        ? "Stop listening"
        : isSpeaking
          ? "Speaking..."
          : "Voice mode on"
      : "Voice mode";

  const micClass = () => {
    if (!voiceSupported) {
      return "text-gray-300 dark:text-neutral-600 cursor-not-allowed";
    }
    if (voiceMode) {
      if (isListening) {
        return "text-red-500 bg-red-50 dark:bg-red-900/20 animate-pulse";
      }
      if (isSpeaking) {
        return "text-blue-500 bg-blue-50 dark:bg-blue-900/20";
      }
      return "text-orange-500 bg-orange-50 dark:bg-orange-900/20";
    }
    return "text-secondary hover:text-primary-600 dark:hover:text-primary-400 hover:bg-black/5 dark:hover:bg-white/5";
  };

  return (
    <ThreadPrimitive.Root className="flex-1 flex flex-col overflow-hidden">
      {/* Scrollable message area */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto"
        role="log"
        aria-live="polite"
      >
        {messages.length === 0 && (
          <div className="flex items-center justify-center h-full">
            <div className="text-center">
              <div className="w-12 h-12 rounded-2xl bg-gradient-to-br from-primary-500 to-primary-700 flex items-center justify-center text-white mx-auto mb-4 shadow-lg shadow-primary-500/20">
                <Send className="w-6 h-6" />
              </div>
              <h2 className="text-lg font-semibold text-primary mb-1">
                Syscity
              </h2>
              <p className="text-secondary text-sm">
                Start a conversation
              </p>
            </div>
          </div>
        )}

        {messages.length > 0 && (
          <div style={{ height: `${totalHeight}px`, position: "relative" }}>
            {virtualItems.map((virtualItem) => (
              <div
                key={virtualItem.key}
                ref={virtualizer.measureElement}
                data-index={virtualItem.index}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${virtualItem.start}px)`,
                }}
              >
                <MessageBubble message={messages[virtualItem.index]} transport={transport} />
              </div>
            ))}
          </div>
        )}
        {isRunning && <MessageSkeleton />}
      </div>

      <div className="bg-page px-4 py-3 shrink-0">
        <ComposerPrimitive.Root className="max-w-[var(--message-list-max-width)] mx-auto w-full">
          <div className="relative flex flex-col rounded-2xl bg-card shadow-sm focus-within:ring-2 focus-within:ring-primary-500/20 transition">
            {/* Command palette */}
            {paletteOpen && (
              <CommandPalette
                commands={paletteCommands}
                selectedIndex={paletteIndex}
                onSelect={handleSelectCommand}
              />
            )}

            {/* Multiline input */}
            <ComposerPrimitive.Input
              ref={inputRef}
              onInput={handleInput}
              onKeyDown={handleKeyDown}
              className="w-full resize-none bg-transparent px-4 pt-3 pb-1 text-sm text-primary placeholder:text-secondary/60 focus:outline-none min-h-[60px] max-h-[200px]"
              placeholder="Message Syscity..."
              rows={1}
              aria-label="Message input"
            />

            {/* Bottom toolbar */}
            <div className="flex items-center justify-between px-2 pb-2 pt-1">
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  title={micTitle}
                  aria-label={micTitle}
                  className={`p-2 rounded-lg transition ${micClass()}`}
                  onClick={toggleVoiceMode}
                  disabled={!voiceSupported}
                >
                  <Mic className="w-5 h-5" />
                </button>
                <button
                  type="button"
                  title="Upload image"
                  aria-label="Upload image"
                  className="p-2 rounded-lg text-secondary hover:text-primary-600 dark:hover:text-primary-400 hover:bg-black/5 dark:hover:bg-white/5 transition"
                  onClick={() => alert("Image upload coming soon")}
                >
                  <Image className="w-5 h-5" />
                </button>
                <button
                  type="button"
                  title="Upload file"
                  aria-label="Upload file"
                  className="p-2 rounded-lg text-secondary hover:text-primary-600 dark:hover:text-primary-400 hover:bg-black/5 dark:hover:bg-white/5 transition"
                  onClick={() => alert("File upload coming soon")}
                >
                  <Paperclip className="w-5 h-5" />
                </button>
              </div>
              {isRunning ? (
                <button
                  type="button"
                  onClick={() => transport.abort()}
                  title="Stop generating"
                  aria-label="Stop generating"
                  className="shrink-0 p-2 rounded-lg bg-red-500 hover:bg-red-600 text-white transition shadow-sm"
                >
                  <Square className="w-4 h-4 fill-current" />
                </button>
              ) : (
                <ComposerPrimitive.Send className="shrink-0 p-2 rounded-lg bg-gradient-to-r from-primary-500 to-primary-700 hover:from-primary-600 hover:to-primary-800 disabled:opacity-40 text-white transition shadow-sm">
                  <Send className="w-4 h-4" />
                </ComposerPrimitive.Send>
              )}
            </div>
          </div>
        </ComposerPrimitive.Root>
      </div>
    </ThreadPrimitive.Root>
  );
}
