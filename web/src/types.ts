export interface MessageType {
  id: string;
  role: 'user' | 'assistant' | 'system' | 'cron';
  content: string;
  timestamp: number;
}

export interface MessageData {
  type: 'system' | 'message' | 'cron' | 'typing' | 'error' | 'version';
  content: string | boolean;
  role?: 'user' | 'assistant';
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
