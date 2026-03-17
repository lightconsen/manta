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
  const [conversationId, setConversationId] = useState<string | null>(null);
  const terminalRef = useRef<HTMLDivElement>(null);
  const wsManagerRef = useRef<WebSocketManager | null>(null);

  // Scroll to bottom when messages change
  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.scrollTop = terminalRef.current.scrollHeight;
    }
  }, [messages, isTyping]);

  // Initialize WebSocket connection with stored conversation ID
  useEffect(() => {
    // Try to get stored conversation ID from localStorage
    const storedConversationId = localStorage.getItem('manta_conversation_id');

    wsManagerRef.current = new WebSocketManager({
      onMessage: handleMessage,
      onStateChange: setWsState,
      onError: (error) => {
        console.error('WebSocket error:', error);
      },
    });

    // Connect with stored conversation ID if available
    wsManagerRef.current.connect(storedConversationId || undefined);
    if (storedConversationId) {
      wsManagerRef.current.setConversationId(storedConversationId);
      setConversationId(storedConversationId);
    }

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
        // Extract conversation ID from system message if present
        if (data.conversation_id) {
          setConversationId(data.conversation_id);
          wsManagerRef.current?.setConversationId(data.conversation_id);
          localStorage.setItem('manta_conversation_id', data.conversation_id);
        }
        break;
      case 'history':
        // Handle history messages from server
        if (data.messages && Array.isArray(data.messages)) {
          const historyMessages = data.messages.map((msg) => ({
            id: msg.id || Date.now().toString() + Math.random(),
            role: msg.role as 'user' | 'assistant' | 'system' | 'cron' | 'tool_call' | 'tool_result',
            content: msg.content,
            timestamp: msg.timestamp ? new Date(msg.timestamp).getTime() : Date.now(),
          }));
          setMessages(historyMessages);
        }
        if (data.conversation_id) {
          setConversationId(data.conversation_id);
          wsManagerRef.current?.setConversationId(data.conversation_id);
          localStorage.setItem('manta_conversation_id', data.conversation_id);
        }
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
      case 'tool_call':
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: 'tool_call',
          content: `🔧 Using tool: ${data.tool}`,
          tool: data.tool,
          arguments: data.arguments,
          timestamp: Date.now(),
        }]);
        break;
      case 'tool_result':
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: 'tool_result',
          content: `✓ Tool result: ${data.result?.substring(0, 200) || 'Done'}`,
          tool: data.tool,
          result: data.result,
          timestamp: Date.now(),
        }]);
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
