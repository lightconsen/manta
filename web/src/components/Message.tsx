import { MessageType } from '../types';
import { formatContent, escapeHtml } from '../utils/format';

interface MessageProps {
  message: MessageType;
}

const avatarMap = {
  user: '👤',
  assistant: '🤖',
  system: 'ℹ️',
  cron: '⏰',
  tool_call: '🔧',
  tool_result: '✓',
};

export function Message({ message }: MessageProps) {
  const { role, content, tool, arguments: args, result } = message;

  // Special rendering for tool calls
  if (role === 'tool_call') {
    return (
      <div className={`message ${role}`}>
        <div className="avatar">{avatarMap[role]}</div>
        <div className="content">
          <div className="tool-name">🔧 Using tool: <strong>{tool}</strong></div>
          {args && (
            <pre className="tool-arguments">
              <code>{args.length > 500 ? args.substring(0, 500) + '...' : args}</code>
            </pre>
          )}
        </div>
      </div>
    );
  }

  // Special rendering for tool results
  if (role === 'tool_result') {
    return (
      <div className={`message ${role}`}>
        <div className="avatar">{avatarMap[role]}</div>
        <div className="content">
          <div className="tool-result-header">✓ Result from <strong>{tool}</strong>:</div>
          {result && (
            <pre className="tool-result">
              <code>{result.length > 500 ? result.substring(0, 500) + '...' : result}</code>
            </pre>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className={`message ${role}`}>
      <div className="avatar">{avatarMap[role]}</div>
      <div
        className="content"
        dangerouslySetInnerHTML={{
          __html: role === 'assistant' ? formatContent(content) : escapeHtml(content)
        }}
      />
    </div>
  );
}
