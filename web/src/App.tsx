import { useState, useEffect, useRef, useCallback } from 'react';
import { MantaLogo } from './components/MantaLogo';
import { Message } from './components/Message';
import { TypingIndicator } from './components/TypingIndicator';
import { Header } from './components/Header';
import { InputArea } from './components/InputArea';
import { SSEManager } from './utils/sse';
import { formatContent, escapeHtml } from './utils/format';
import { MessageType, MessageData, ConnectionState } from './types';
import './styles.css';

function App() {
  const [messages, setMessages] = useState<MessageType[]>([]);
  const [isTyping, setIsTyping] = useState(false);
  const [connectionState, setConnectionState] = useState<ConnectionState>(ConnectionState.Connecting);
  const [version, setVersion] = useState('v0.1.0');
  const [conversationId, setConversationId] = useState<string | null>(null);
  const terminalRef = useRef<HTMLDivElement>(null);
  const sseManagerRef = useRef<SSEManager | null>(null);

  // Scroll to bottom when messages change
  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.scrollTop = terminalRef.current.scrollHeight;
    }
  }, [messages, isTyping]);

  // Fetch conversation history from API
  const fetchHistory = useCallback(async (convId: string) => {
    try {
      const response = await fetch(`/api/v1/conversations/${convId}/messages?limit=100`);
      if (!response.ok) {
        console.error('Failed to fetch history:', response.statusText);
        return;
      }
      const data = await response.json();
      if (data.messages && Array.isArray(data.messages)) {
        const historyMessages = data.messages.map((msg: any) => ({
          id: msg.id || Date.now().toString() + Math.random(),
          role: msg.role as 'user' | 'assistant' | 'system' | 'cron' | 'tool_call' | 'tool_result',
          content: msg.content,
          timestamp: msg.created_at ? new Date(msg.created_at).getTime() : Date.now(),
        }));
        setMessages(historyMessages);
      }
    } catch (err) {
      console.error('Error fetching history:', err);
    }
  }, []);

  // Initialize SSE connection with stored conversation ID
  useEffect(() => {
    // Try to get stored conversation ID from localStorage
    const storedConversationId = localStorage.getItem('manta_conversation_id');

    sseManagerRef.current = new SSEManager({
      onMessage: handleMessage,
      onStateChange: setConnectionState,
      onError: (error) => {
        console.error('SSE error:', error);
      },
      onConversationId: (id) => {
        setConversationId(id);
        sseManagerRef.current?.setConversationId(id);
        localStorage.setItem('manta_conversation_id', id);
      },
    });

    // Connect with stored conversation ID if available
    sseManagerRef.current.connect(storedConversationId || undefined);
    if (storedConversationId) {
      sseManagerRef.current.setConversationId(storedConversationId);
      setConversationId(storedConversationId);
      // Fetch history for the stored conversation
      fetchHistory(storedConversationId);
    }

    return () => {
      sseManagerRef.current?.disconnect();
    };
  }, [fetchHistory]);

  const handleMessage = useCallback((data: MessageData) => {
    // Handle new GatewayEvent format with event_type field
    const eventType = data.event_type || data.type;

    switch (eventType) {
      case 'agent_response':
        // Extract content from AgentResponse event
        const agentContent = data.AgentResponse?.content || data.content;
        if (agentContent) {
          setMessages((prev) => [...prev, {
            id: Date.now().toString(),
            role: 'assistant',
            content: agentContent,
            timestamp: Date.now(),
          }]);
        }
        setIsTyping(false);
        break;
      case 'thinking':
        setIsTyping(true);
        break;
      case 'tool_calling':
        const toolName = data.ToolCalling?.tool_name || data.tool;
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: 'tool_call',
          content: `🔧 Using tool: ${toolName}...`,
          tool: toolName,
          arguments: data.ToolCalling?.arguments || data.arguments,
          timestamp: Date.now(),
        }]);
        break;
      case 'tool_result':
        const resultToolName = data.ToolResult?.tool_name || data.tool;
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: 'tool_result',
          content: `✓ Tool ${resultToolName} completed`,
          tool: resultToolName,
          result: data.ToolResult?.result || data.result,
          timestamp: Date.now(),
        }]);
        break;
      case 'agent_status':
        const status = data.AgentStatus?.status;
        if (status === 'Idle') {
          setIsTyping(false);
        } else if (status?.Processing) {
          setIsTyping(true);
        }
        break;
      case 'processing_error':
        const errorMsg = data.ProcessingError?.message || data.content;
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: 'system',
          content: `Error: ${errorMsg}`,
          timestamp: Date.now(),
        }]);
        setIsTyping(false);
        break;
      case 'system':
        setMessages((prev) => [...prev, {
          id: Date.now().toString(),
          role: 'system',
          content: data.content,
          timestamp: Date.now(),
        }]);
        if (data.conversation_id) {
          setConversationId(data.conversation_id);
          sseManagerRef.current?.setConversationId(data.conversation_id);
          localStorage.setItem('manta_conversation_id', data.conversation_id);
        }
        break;
      case 'history':
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
          sseManagerRef.current?.setConversationId(data.conversation_id);
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
        setIsTyping(false);
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
      case 'version':
        if (typeof data.content === 'string') {
          setVersion(data.content);
        }
        break;
    }
  }, []);

  const handleSendMessage = useCallback(async (text: string) => {
    if (!text.trim()) return;

    // Add user message to UI immediately
    setMessages((prev) => [...prev, {
      id: Date.now().toString(),
      role: 'user',
      content: text,
      timestamp: Date.now(),
    }]);

    // Send to server via POST
    try {
      await sseManagerRef.current?.send(text);
    } catch (err) {
      console.error('Failed to send message:', err);
      setMessages((prev) => [...prev, {
        id: Date.now().toString(),
        role: 'system',
        content: 'Failed to send message. Please try again.',
        timestamp: Date.now(),
      }]);
    }
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
        connectionState={connectionState}
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
        disabled={connectionState !== ConnectionState.Connected}
      />
    </>
  );
}

export default App;
