import { useState, useEffect, useRef, useCallback } from 'react';
import { MantaLogo } from './components/MantaLogo';
import { Message } from './components/Message';
import { TypingIndicator } from './components/TypingIndicator';
import { Header } from './components/Header';
import { InputArea } from './components/InputArea';
import { WebSocketManager } from './utils/websocket';
import { formatContent, escapeHtml } from './utils/format';
import { MessageType, MessageData, WebSocketState } from './types';
import './styles.css';

function App() {
  const [messages, setMessages] = useState<MessageType[]>([]);
  const [isTyping, setIsTyping] = useState(false);
  const [wsState, setWsState] = useState<WebSocketState>(WebSocketState.Connecting);
  const [version, setVersion] = useState('v0.1.0');
  const terminalRef = useRef<HTMLDivElement>(null);
  const wsManagerRef = useRef<WebSocketManager | null>(null);

  // Scroll to bottom when messages change
  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.scrollTop = terminalRef.current.scrollHeight;
    }
  }, [messages, isTyping]);

  // Initialize WebSocket connection
  useEffect(() => {
    wsManagerRef.current = new WebSocketManager({
      onMessage: handleMessage,
      onStateChange: setWsState,
      onError: (error) => {
        console.error('WebSocket error:', error);
      },
    });

    wsManagerRef.current.connect();

    return () => {
      wsManagerRef.current?.disconnect();
    };
  }, []);

  const handleMessage = useCallback((data: MessageData) => {
    switch (data.type) {
      case 'system':
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: 'system',
          content: data.content,
          timestamp: Date.now(),
        }]);
        break;
      case 'message':
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: data.role || 'assistant',
          content: data.content,
          timestamp: Date.now(),
        }]);
        break;
      case 'cron':
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: 'cron',
          content: data.content,
          timestamp: Date.now(),
        }]);
        break;
      case 'typing':
        setIsTyping(data.content === true);
        break;
      case 'version':
        if (typeof data.content === 'string') {
          setVersion(data.content);
        }
        break;
    }
  }, []);

  const handleSendMessage = useCallback((text: string) => {
    if (!text.trim() || !wsManagerRef.current?.isConnected()) return;

    // Add user message to UI
    setMessages((prev) => [...prev, {
      id: Date.now().toString(),
      role: 'user',
      content: text,
      timestamp: Date.now(),
    }]);

    // Send to server
    wsManagerRef.current.send(text);
  }, []);

  const handleSettingsClick = useCallback(() => {
    setMessages((prev) => [...prev, {
      id: Date.now().toString(),
      role: 'system',
      content: 'Settings panel coming soon! 🚧',
      timestamp: Date.now(),
    }]);
  }, []);

  return (
    <>
      <Header
        logo={<MantaLogo />}
        wsState={wsState}
        version={version}
        onSettingsClick={handleSettingsClick}
      />

      <div className="terminal" ref={terminalRef}>
        {messages.map((msg) => (
          <Message key={msg.id} message={msg} />
        ))}
        {isTyping && <TypingIndicator />}
      </div>

      <InputArea
        onSendMessage={handleSendMessage}
        disabled={wsState !== WebSocketState.Connected}
      />
    </>
  );
}

export default App;
