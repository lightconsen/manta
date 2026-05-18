import { useState, useRef, useEffect, useCallback } from "react";
import { MantaWebSocketTransport, WsEvent } from "./MantaWebSocketTransport";

interface Message {
  id: string;
  role: "user" | "assistant";
  content: string;
  status?: "streaming" | "done" | "error";
}

function generateId(): string {
  return "msg_" + Math.random().toString(36).slice(2, 10);
}

export function ChatUI({ transport }: { transport: MantaWebSocketTransport }) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [connectionStatus, setConnectionStatus] = useState<"connecting" | "connected" | "disconnected">("connecting");
  const scrollRef = useRef<HTMLDivElement>(null);
  const streamingRef = useRef<boolean>(false);

  const scrollToBottom = useCallback(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, []);

  useEffect(() => {
    scrollToBottom();
  }, [messages, scrollToBottom]);

  useEffect(() => {
    const checkConnection = setInterval(() => {
      // Simple heuristic: if we haven't seen an error/reconnect, assume connected
      // In a real app we'd track ws.readyState
      setConnectionStatus("connected");
    }, 2000);
    return () => clearInterval(checkConnection);
  }, []);

  useEffect(() => {
    const unsubscribe = transport.onEvent((evt: WsEvent) => {
      if (evt.event === "chat.delta") {
        const delta = (evt.payload?.content as string) || "";
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (last && last.role === "assistant" && last.status === "streaming") {
            const updated = [...prev];
            updated[updated.length - 1] = { ...last, content: last.content + delta };
            return updated;
          }
          return prev;
        });
      } else if (evt.event === "chat.final") {
        const response = (evt.payload?.response as string) || "";
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (last && last.role === "assistant" && last.status === "streaming") {
            const updated = [...prev];
            updated[updated.length - 1] = { ...last, content: response, status: "done" };
            return updated;
          }
          return prev;
        });
        streamingRef.current = false;
        setIsLoading(false);
      } else if (evt.event === "chat.error") {
        const errorMsg = (evt.payload?.message as string) || "Chat error";
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (last && last.role === "assistant" && last.status === "streaming") {
            const updated = [...prev];
            updated[updated.length - 1] = { ...last, content: "Error: " + errorMsg, status: "error" };
            return updated;
          }
          return prev;
        });
        streamingRef.current = false;
        setIsLoading(false);
      }
    });
    return unsubscribe;
  }, [transport]);

  const handleSend = async () => {
    const text = input.trim();
    if (!text || isLoading || streamingRef.current) return;

    setInput("");
    setMessages((prev) => [
      ...prev,
      { id: generateId(), role: "user", content: text },
      { id: generateId(), role: "assistant", content: "", status: "streaming" },
    ]);
    setIsLoading(true);
    streamingRef.current = true;

    transport.sendMessage(text);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleNewChat = () => {
    transport.createSession();
    setMessages([]);
    setIsLoading(false);
    streamingRef.current = false;
  };

  return (
    <div className="h-screen flex flex-col bg-white dark:bg-neutral-900">
      <header className="flex items-center justify-between border-b border-gray-200 dark:border-neutral-700 px-4 h-12 shrink-0">
        <div className="flex items-center gap-2">
          <h1 className="text-sm font-semibold text-gray-900 dark:text-gray-100">
            Manta
          </h1>
          <span
            className={`inline-block w-2 h-2 rounded-full ${
              connectionStatus === "connected"
                ? "bg-green-500"
                : connectionStatus === "connecting"
                ? "bg-yellow-500"
                : "bg-red-500"
            }`}
          />
        </div>
        <button
          onClick={handleNewChat}
          className="text-xs px-3 py-1.5 rounded-md bg-gray-100 dark:bg-neutral-800 hover:bg-gray-200 dark:hover:bg-neutral-700 text-gray-700 dark:text-gray-300 transition"
        >
          New Chat
        </button>
      </header>

      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto px-4 py-6 space-y-6"
      >
        {messages.length === 0 && (
          <div className="flex items-center justify-center h-full">
            <p className="text-gray-400 dark:text-neutral-500 text-sm">
              Start a conversation with Manta
            </p>
          </div>
        )}
        {messages.map((msg) => (
          <div
            key={msg.id}
            className={`flex ${
              msg.role === "user" ? "justify-end" : "justify-start"
            }`}
          >
            <div
              className={`max-w-[80%] px-4 py-2.5 rounded-2xl text-sm leading-relaxed ${
                msg.role === "user"
                  ? "bg-blue-600 text-white rounded-br-md"
                  : "bg-gray-100 dark:bg-neutral-800 text-gray-900 dark:text-gray-100 rounded-bl-md"
              }`}
            >
              {msg.content || (msg.status === "streaming" ? (
                <span className="inline-flex gap-1">
                  <span className="w-1.5 h-1.5 bg-gray-400 dark:bg-neutral-500 rounded-full animate-bounce" style={{ animationDelay: "0ms" }} />
                  <span className="w-1.5 h-1.5 bg-gray-400 dark:bg-neutral-500 rounded-full animate-bounce" style={{ animationDelay: "150ms" }} />
                  <span className="w-1.5 h-1.5 bg-gray-400 dark:bg-neutral-500 rounded-full animate-bounce" style={{ animationDelay: "300ms" }} />
                </span>
              ) : null)}
            </div>
          </div>
        ))}
      </div>

      <div className="border-t border-gray-200 dark:border-neutral-700 px-4 py-3 shrink-0">
        <div className="flex items-end gap-2 max-w-3xl mx-auto">
          <textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Type a message..."
            rows={1}
            className="flex-1 resize-none rounded-xl border border-gray-300 dark:border-neutral-600 bg-white dark:bg-neutral-800 px-4 py-2.5 text-sm text-gray-900 dark:text-gray-100 placeholder-gray-400 dark:placeholder-neutral-500 focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 transition"
            style={{ minHeight: "44px", maxHeight: "120px" }}
          />
          <button
            onClick={handleSend}
            disabled={!input.trim() || isLoading}
            className="shrink-0 px-4 py-2.5 rounded-xl bg-blue-600 hover:bg-blue-700 disabled:bg-gray-300 dark:disabled:bg-neutral-700 text-white text-sm font-medium transition"
          >
            Send
          </button>
        </div>
      </div>
    </div>
  );
}
