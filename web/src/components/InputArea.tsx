import { useState, useCallback } from 'react';

interface InputAreaProps {
  onSendMessage: (text: string) => void;
  disabled: boolean;
}

export function InputArea({ onSendMessage, disabled }: InputAreaProps) {
  const [text, setText] = useState('');

  const handleSubmit = useCallback(() => {
    if (!text.trim() || disabled) return;
    onSendMessage(text);
    setText('');
  }, [text, disabled, onSendMessage]);

  const handleKeyPress = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      handleSubmit();
    }
  }, [handleSubmit]);

  return (
    <div className="input-area">
      <div className="input-wrapper">
        <span className="prompt">💬</span>
        <input
          type="text"
          id="messageInput"
          placeholder="Type your message..."
          autoComplete="off"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyPress={handleKeyPress}
          disabled={disabled}
        />
      </div>
      <button id="sendButton" onClick={handleSubmit} disabled={disabled}>
        Send
      </button>
    </div>
  );
}
