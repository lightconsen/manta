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
};

export function Message({ message }: MessageProps) {
  const { role, content } = message;

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
