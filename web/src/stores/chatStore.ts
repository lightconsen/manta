import { create } from "zustand";
import type { ChatMessage, NetworkStatus } from "@/SyscityWebSocketTransport";

interface ChatState {
  messages: ChatMessage[];
  sessions: Array<{
    id: string;
    label?: string;
    pinned?: boolean;
    last_activity?: number;
  }>;
  currentSessionId: string;
  currentAgent?: {
    id: string;
    display_name: string;
    emoji: string;
  };
  networkStatus: NetworkStatus;
  isRunning: boolean;
  voiceMode: boolean;
  isLoadingHistory: boolean;
  hasMoreHistory: boolean;
  aiInternalsVisibility: Record<string, boolean>;

  setMessages: (messages: ChatMessage[]) => void;
  prependMessages: (messages: ChatMessage[]) => void;
  appendMessage: (message: ChatMessage) => void;
  updateMessage: (id: string, updater: (msg: ChatMessage) => ChatMessage) => void;
  setSessions: (
    sessions: Array<{
      id: string;
      label?: string;
      pinned?: boolean;
      last_activity?: number;
    }>
  ) => void;
  setCurrentSessionId: (id: string) => void;
  setCurrentAgent: (agent?: { id: string; display_name: string; emoji: string }) => void;
  setNetworkStatus: (status: NetworkStatus) => void;
  setIsRunning: (running: boolean) => void;
  setVoiceMode: (enabled: boolean) => void;
  setIsLoadingHistory: (loading: boolean) => void;
  setHasMoreHistory: (hasMore: boolean) => void;
  setAiInternalsVisibility: (messageId: string, visible: boolean) => void;
}

export const useChatStore = create<ChatState>((set) => ({
  messages: [],
  sessions: [],
  currentSessionId: "",
  networkStatus: "connecting",
  isRunning: false,
  voiceMode: false,
  isLoadingHistory: false,
  hasMoreHistory: false,
  aiInternalsVisibility: {},

  setMessages: (messages) => set({ messages }),
  prependMessages: (messages) => set((s) => ({ messages: [...messages, ...s.messages] })),
  appendMessage: (message) => set((s) => ({ messages: [...s.messages, message] })),
  updateMessage: (id, updater) =>
    set((s) => ({
      messages: s.messages.map((m) => (m.id === id ? updater(m) : m)),
    })),
  setSessions: (sessions) => set({ sessions }),
  setCurrentSessionId: (id) => set({ currentSessionId: id }),
  setCurrentAgent: (agent) => set({ currentAgent: agent }),
  setNetworkStatus: (status) => set({ networkStatus: status }),
  setIsRunning: (running) => set({ isRunning: running }),
  setVoiceMode: (voiceMode) => set({ voiceMode }),
  setIsLoadingHistory: (isLoadingHistory) => set({ isLoadingHistory }),
  setHasMoreHistory: (hasMoreHistory) => set({ hasMoreHistory }),
  setAiInternalsVisibility: (messageId, visible) =>
    set((s) => ({
      aiInternalsVisibility: {
        ...s.aiInternalsVisibility,
        [messageId]: visible,
      },
    })),
}));
