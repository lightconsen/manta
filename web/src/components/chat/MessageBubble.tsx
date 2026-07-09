import { MarkdownMessage } from "@/components/shared/MarkdownMessage";
import { ReasoningPart } from "@/components/shared/ReasoningPart";
import { ToolCallPart } from "@/components/shared/ToolCallPart";
import { Avatar } from "./Avatar";
import { LiveStatusBar } from "./LiveStatusBar";
import { Clock, Wrench } from "lucide-react";
import { formatDuration } from "@/lib/utils";
import type { ChatMessage, SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";

interface MessageBubbleProps {
  message: ChatMessage;
  transport?: SyscityWebSocketTransport;
}

const centerStyle: Record<string, string> = {
  paddingLeft: "calc((100% - var(--message-list-max-width)) / 2)",
  paddingRight: "calc((100% - var(--message-list-max-width)) / 2)",
};

export function MessageBubble({ message, transport }: MessageBubbleProps) {
  const isUser = message.role === "user";

  if (isUser) {
    return (
      <div className="py-4">
        <div className="flex gap-3 flex-row-reverse" style={centerStyle}>
          <Avatar role="user" />
          <div className="flex-1 min-w-0 text-right">
            <div className="text-[11px] font-medium text-secondary mb-1 uppercase tracking-wide">
              You
            </div>
            <div className="inline-block text-left rounded-2xl px-4 py-2.5 bg-primary-600 text-white rounded-br-md">
              <p className="text-sm leading-relaxed whitespace-pre-wrap">
                {message.content}
              </p>
            </div>
          </div>
        </div>
      </div>
    );
  }

  const hasParts = message.parts && message.parts.length > 0;
  const isAssistant = message.role === "assistant";
  const hasMetadata = isAssistant && (message.durationMs !== undefined || message.toolCount !== undefined);

  return (
    <div className="py-4">
      <div className="flex gap-3 flex-row" style={centerStyle}>
        <Avatar role="assistant" />
        <div className="flex-1 min-w-0">
          <div className="text-[11px] font-medium text-secondary mb-1 uppercase tracking-wide">
            Syscity
          </div>
          {hasParts ? (
            <div className="space-y-1">
              {message.parts!.map((part, i) => {
                if (part.type === "reasoning") {
                  return <ReasoningPart key={i} text={part.text || ""} />;
                }
                if (part.type === "tool-call") {
                  return (
                    <ToolCallPart
                      key={i}
                      toolName={part.toolName || "tool"}
                      args={part.args || {}}
                      result={part.result}
                      data={part.data}
                      transport={transport}
                    />
                  );
                }
                if (part.type === "text") {
                  return (
                    <div className="text-primary">
                      <MarkdownMessage text={part.text || ""} />
                    </div>
                  );
                }
                return null;
              })}
            </div>
          ) : (
            <div className="text-primary">
              <MarkdownMessage text={message.content} />
            </div>
          )}
          {/* Live status or metadata footer */}
          {message.liveStatus && (
            <LiveStatusBar
              liveStatus={message.liveStatus}
              startTime={message.timestamp ?? Date.now() - 5000}
            />
          )}
          {!message.liveStatus && hasMetadata && (
            <div className="mt-1.5 flex items-center gap-3 text-[10px] text-secondary">
              {message.durationMs !== undefined && (
                <span className="flex items-center gap-1">
                  <Clock className="w-3 h-3" />
                  {formatDuration(message.durationMs)}
                </span>
              )}
              {message.toolCount !== undefined && message.toolCount > 0 && (
                <span className="flex items-center gap-1">
                  <Wrench className="w-3 h-3" />
                  {message.toolCount} tool{message.toolCount !== 1 ? "s" : ""}
                </span>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
