import { useState, useCallback, useRef, useEffect } from 'react';

interface InputAreaProps {
  onSendMessage: (text: string) => void;
  disabled: boolean;
}

export function InputArea({ onSendMessage, disabled }: InputAreaProps) {
  const [text, setText] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Auto-resize textarea
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight, 120) + 'px';
  }, [text]);

  const handleSubmit = useCallback(() => {
    if (!text.trim() || disabled) return;
    onSendMessage(text);
    setText('');
    // Reset height
    const el = textareaRef.current;
    if (el) el.style.height = 'auto';
  }, [text, disabled, onSendMessage]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  }, [handleSubmit]);

  return (
    <div className="input-area">
      <div className="input-area-inner">
        <div className="input-wrapper">
          <textarea
            ref={textareaRef}
            id="messageInput"
            placeholder="输入消息..."
            autoComplete="off"
            rows={1}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={disabled}
          />
        </div>
        <button id="sendButton" onClick={handleSubmit} disabled={disabled || !text.trim()}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="22" y1="2" x2="11" y2="13" />
            <polygon points="22 2 15 22 11 13 2 9 22 2" />
          </svg>
        </button>
      </div>
      <div className="input-hint">Enter 发送 · Shift+Enter 换行</div>
    </div>
  );
}
