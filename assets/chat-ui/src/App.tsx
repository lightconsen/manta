import { useMemo } from "react";
import {
  AssistantRuntimeProvider,
  ThreadPrimitive,
  ComposerPrimitive,
  MessagePrimitive,
  useLocalRuntime,
  AuiIf,
} from "@assistant-ui/react";
import { MantaWebSocketTransport } from "./MantaWebSocketTransport";
import { TextPart } from "./components/TextPart";
import { ReasoningPart } from "./components/ReasoningPart";
import { ToolCallPart } from "./components/ToolCallPart";

function ChatApp() {
  const transport = useMemo(() => new MantaWebSocketTransport(), []);
  const runtime = useLocalRuntime(transport);

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <div className="h-screen flex flex-col bg-white dark:bg-neutral-900">
        <header className="flex items-center justify-between border-b border-gray-200 dark:border-neutral-700 px-4 h-12 shrink-0">
          <h1 className="text-sm font-semibold text-gray-900 dark:text-gray-100">
            Manta
          </h1>
          <button
            onClick={() => {
              transport.createSession();
              window.location.reload();
            }}
            className="text-xs px-3 py-1 rounded bg-gray-100 dark:bg-neutral-800 hover:bg-gray-200 dark:hover:bg-neutral-700 transition"
          >
            New Chat
          </button>
        </header>

        <ThreadPrimitive.Root className="flex-1 flex flex-col overflow-hidden">
          <ThreadPrimitive.Viewport className="flex-1 overflow-y-auto px-4 py-4">
            <ThreadPrimitive.Messages>
              {({ message }) => (
                <div
                  className={`flex ${
                    message.role === "user" ? "justify-end" : "justify-start"
                  }`}
                >
                  <div
                    className={`max-w-[80%] px-4 py-2.5 rounded-2xl text-sm leading-relaxed ${
                      message.role === "user"
                        ? "bg-blue-600 text-white rounded-br-md"
                        : "bg-gray-100 dark:bg-neutral-800 text-gray-900 dark:text-gray-100 rounded-bl-md"
                    }`}
                  >
                    {message.role === "user" ? (
                      <p>
                        {message.content
                          .map((c) =>
                            c.type === "text" ? c.text : ""
                          )
                          .join("")}
                      </p>
                    ) : (
                      <MessagePrimitive.Root asChild>
                        <div>
                          <MessagePrimitive.Content
                            components={{
                              Text: TextPart,
                              Reasoning: ReasoningPart,
                              tools: {
                                Fallback: ToolCallPart,
                              },
                            }}
                          />
                        </div>
                      </MessagePrimitive.Root>
                    )}
                  </div>
                </div>
              )}
            </ThreadPrimitive.Messages>

            <AuiIf condition={(s) => s.thread.isEmpty}>
              <div className="flex items-center justify-center h-full">
                <p className="text-gray-400 dark:text-neutral-500 text-sm">
                  Start a conversation with Manta
                </p>
              </div>
            </AuiIf>
          </ThreadPrimitive.Viewport>

          <div className="border-t border-gray-200 dark:border-neutral-700 px-4 py-3 shrink-0">
            <ComposerPrimitive.Root className="flex items-end gap-2 max-w-3xl mx-auto">
              <ComposerPrimitive.Input
                className="flex-1 resize-none rounded-xl border border-gray-300 dark:border-neutral-600 bg-white dark:bg-neutral-800 px-4 py-2.5 text-sm text-gray-900 dark:text-gray-100 placeholder-gray-400 dark:placeholder-neutral-500 focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 transition min-h-[44px] max-h-[120px]"
                placeholder="Type a message..."
              />
              <ComposerPrimitive.Send className="shrink-0 px-4 py-2.5 rounded-xl bg-blue-600 hover:bg-blue-700 disabled:bg-gray-300 dark:disabled:bg-neutral-700 text-white text-sm font-medium transition">
                Send
              </ComposerPrimitive.Send>
            </ComposerPrimitive.Root>
          </div>
        </ThreadPrimitive.Root>
      </div>
    </AssistantRuntimeProvider>
  );
}

export default ChatApp;
