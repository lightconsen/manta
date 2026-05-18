import { useState, useCallback } from 'react';
import { MessageType } from '../types';
import { formatContent } from '../utils/format';

interface MessageProps {
  message: MessageType;
}

const avatarMap: Record<string, string> = {
  user: 'U',
  assistant: 'AI',
  system: 'ℹ',
  cron: '⏰',
  tool_call: '🔧',
  tool_result: '✓',
};

function formatTime(timestamp: number): string {
  const d = new Date(timestamp);
  return d.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
  });
}

function stripHtml(html: string): string {
  const tmp = document.createElement('div');
  tmp.innerHTML = html;
  return tmp.textContent || tmp.innerText || '';
}

export function Message({ message }: MessageProps) {
  const { role, content, tool, arguments: args, result, timestamp } = message;
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(() => {
    let text = content;
    if (role === 'tool_call' && args) {
      text = `${tool}\n${args}`;
    } else if (role === 'tool_result' && result) {
      text = `${tool}\n${result}`;
    }
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  }, [content, role, tool, args, result]);

  // System messages are minimal
  if (role === 'system') {
    return (
      <div className="message-row system">
        <div className="message-bubble">
          {content}
        </div>
        <button
          className={`message-copy-btn ${copied ? 'copied' : ''}`}
          onClick={handleCopy}
          title={copied ? '已复制' : '复制'}
        >
          {copied ? '✓' : '⎘'}
        </button>
      </div>
    );
  }

  // Tool call
  if (role === 'tool_call') {
    return (
      <div className="message-row tool_call">
        <div className="message-avatar">{avatarMap[role]}</div>
        <div>
          <div className="message-bubble">
            <div className="tool-name">🔧 {tool}</div>
            {args && (
              <pre className="tool-arguments">
                <code>{args.length > 800 ? args.substring(0, 800) + '...' : args}</code>
              </pre>
            )}
          </div>
          <div className="message-timestamp">{formatTime(timestamp)}</div>
          <button
            className={`message-copy-btn ${copied ? 'copied' : ''}`}
            onClick={handleCopy}
            title={copied ? '已复制' : '复制'}
          >
            {copied ? '✓' : '⎘'}
          </button>
        </div>
      </div>
    );
  }

  // Tool result
  if (role === 'tool_result') {
    return (
      <div className="message-row tool_result">
        <div className="message-avatar">{avatarMap[role]}</div>
        <div>
          <div className="message-bubble">
            <div className="tool-result-header">✓ {tool}</div>
            {result && (
              <pre className="tool-result">
                <code>{result.length > 800 ? result.substring(0, 800) + '...' : result}</code>
              </pre>
            )}
          </div>
          <div className="message-timestamp">{formatTime(timestamp)}</div>
          <button
            className={`message-copy-btn ${copied ? 'copied' : ''}`}
            onClick={handleCopy}
            title={copied ? '已复制' : '复制'}
          >
            {copied ? '✓' : '⎘'}
          </button>
        </div>
      </div>
    );
  }

  // Regular user / assistant / cron
  const html = role === 'assistant' || role === 'cron'
    ? formatContent(content)
    : content;

  return (
    <div className={`message-row ${role}`}>
      <div className="message-avatar">{avatarMap[role]}</div>
      <div style={{ position: 'relative', maxWidth: '85%' }}>
        <div
          className="message-bubble"
          dangerouslySetInnerHTML={{ __html: html }}
        />
        <div className="message-timestamp">{formatTime(timestamp)}</div>
        <button
          className={`message-copy-btn ${copied ? 'copied' : ''}`}
          onClick={handleCopy}
          title={copied ? '已复制' : '复制'}
        >
          {copied ? '✓' : '⎘'}
        </button>
      </div>
    </div>
  );
}
