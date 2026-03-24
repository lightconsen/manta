export interface MessageType {
  id: string;
  role: 'user' | 'assistant' | 'system' | 'cron' | 'tool_call' | 'tool_result';
  content: string;
  timestamp: number;
  tool?: string;
  arguments?: string;
  result?: string;
}

export interface MessageData {
  type?: 'system' | 'message' | 'cron' | 'typing' | 'error' | 'version' | 'tool_call' | 'tool_result' | 'history';
  event_type?: 'agent_response' | 'thinking' | 'tool_calling' | 'tool_result' | 'agent_status' | 'processing_error' | 'completed' | 'message_received' | 'channel_status' | 'approval_required' | 'repair_action' | 'cron_announce' | 'system' | 'message' | 'cron' | 'typing' | 'error' | 'version' | 'tool_call' | 'tool_result' | 'history';
  content: string | boolean;
  role?: 'user' | 'assistant';
  tool?: string;
  arguments?: string;
  result?: string;
  conversation_id?: string;
  // GatewayEvent nested data
  AgentResponse?: {
    agent_id: string;
    content: string;
    channel: string;
    conversation_id: string;
    session_id: string;
    usage?: any;
  };
  Thinking?: {
    agent_id: string;
    content?: string;
    session_id: string;
  };
  ToolCalling?: {
    agent_id: string;
    tool_name: string;
    arguments: string;
    session_id: string;
  };
  ToolResult?: {
    agent_id: string;
    tool_name: string;
    result: string;
    session_id: string;
  };
  AgentStatus?: {
    agent_id: string;
    status: string | { Processing?: { session_id: string } };
  };
  ProcessingError?: {
    agent_id: string;
    message: string;
    session_id: string;
  };
  messages?: Array<{
    id: string;
    role: string;
    content: string;
    timestamp?: string;
  }>;
}

export enum WebSocketState {
  Connecting = 'connecting',
  Connected = 'connected',
  Disconnected = 'disconnected',
}

export interface WebSocketConfig {
  onMessage: (data: MessageData) => void;
  onStateChange: (state: WebSocketState) => void;
  onError?: (error: Event) => void;
}

/**
 * SSE (Server-Sent Events) connection states
 */
export enum ConnectionState {
  Connecting = 'connecting',
  Connected = 'connected',
  Disconnected = 'disconnected',
  Reconnecting = 'reconnecting',
  Error = 'error',
}

/**
 * SSE Manager configuration
 */
export interface SSEConfig {
  onMessage: (data: MessageData) => void;
  onStateChange: (state: ConnectionState) => void;
  onError?: (error: Event | Error) => void;
  onConversationId?: (id: string) => void;
}
