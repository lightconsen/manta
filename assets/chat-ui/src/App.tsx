import { useMemo } from "react";
import {
  AssistantRuntimeProvider,
  Thread,
  useChatRuntime,
} from "@assistant-ui/react";
import { MantaWebSocketTransport } from "./MantaWebSocketTransport";

function ChatApp() {
  const transport = useMemo(() => new MantaWebSocketTransport(), []);
  const runtime = useChatRuntime({
    adapters: {
      chatModel: transport,
    },
  });

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <div className="h-screen flex flex-col bg-white dark:bg-neutral-900">
        <header className="flex items-center justify-between border-b border-gray-200 dark:border-neutral-700 px-4 h-12">
          <h1 className="text-sm font-semibold text-gray-900 dark:text-gray-100">
            Manta
          </h1>
          <button
            onClick={() => {
              localStorage.removeItem("manta_session");
              window.location.reload();
            }}
            className="text-xs px-3 py-1 rounded bg-gray-100 dark:bg-neutral-800 hover:bg-gray-200 dark:hover:bg-neutral-700 transition"
          >
            New Chat
          </button>
        </header>
        <div className="flex-1 overflow-hidden">
          <Thread />
        </div>
      </div>
    </AssistantRuntimeProvider>
  );
}

export default ChatApp;
