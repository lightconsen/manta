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
  type: 'system' | 'message' | 'cron' | 'typing' | 'error' | 'version' | 'tool_call' | 'tool_result';
  content: string | boolean;
  role?: 'user' | 'assistant';
  tool?: string;
  arguments?: string;
  result?: string;
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
