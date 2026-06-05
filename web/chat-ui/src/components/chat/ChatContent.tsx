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
              <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100 mb-1">
                Syscity
              </h2>
              <p className="text-gray-400 dark:text-neutral-500 text-sm">
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
                <MessageBubble message={messages[virtualItem.index]} />
              </div>
            ))}
          </div>
        )}
        {isRunning && <MessageSkeleton />}
      </div>

      <div className="bg-white dark:bg-neutral-900 px-4 py-3 shrink-0">
        <ComposerPrimitive.Root className="max-w-3xl mx-auto w-full">
          <div className="relative flex flex-col rounded-xl border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 focus-within:ring-2 focus-within:ring-primary-500/30 focus-within:border-primary-500/50 transition">
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
              className="w-full resize-none bg-transparent px-4 pt-3 pb-1 text-sm text-gray-900 dark:text-gray-100 placeholder-gray-400 dark:placeholder-neutral-500 focus:outline-none min-h-[60px] max-h-[200px]"
              placeholder="Message Syscity..."
              rows={1}
              aria-label="Message input"
            />

            {/* Bottom toolbar */}
            <div className="flex items-center justify-between px-2 pb-2 pt-1">
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  title="Voice input"
                  aria-label="Voice input"
                  className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-primary-500 dark:hover:text-primary-400 hover:bg-gray-100 dark:hover:bg-neutral-700/50 transition"
                  onClick={() => alert("Voice input coming soon")}
                >
                  <Mic className="w-5 h-5" />
                </button>
                <button
                  type="button"
                  title="Upload image"
                  aria-label="Upload image"
                  className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-primary-500 dark:hover:text-primary-400 hover:bg-gray-100 dark:hover:bg-neutral-700/50 transition"
                  onClick={() => alert("Image upload coming soon")}
                >
                  <Image className="w-5 h-5" />
                </button>
                <button
                  type="button"
                  title="Upload file"
                  aria-label="Upload file"
                  className="p-2 rounded-lg text-gray-400 dark:text-neutral-500 hover:text-primary-500 dark:hover:text-primary-400 hover:bg-gray-100 dark:hover:bg-neutral-700/50 transition"
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
