import { create } from "zustand";
import type { ChatMessage, NetworkStatus } from "@/SyscityWebSocketTransport";

interface ChatState {
  messages: ChatMessage[];
  sessions: Array<{ id: string; label?: string }>;
  currentSessionId: string;
  networkStatus: NetworkStatus;
  isRunning: boolean;
  voiceMode: boolean;

  setMessages: (messages: ChatMessage[]) => void;
  appendMessage: (message: ChatMessage) => void;
  updateMessage: (id: string, updater: (msg: ChatMessage) => ChatMessage) => void;
  setSessions: (sessions: Array<{ id: string; label?: string }>) => void;
  setCurrentSessionId: (id: string) => void;
  setNetworkStatus: (status: NetworkStatus) => void;
  setIsRunning: (running: boolean) => void;
  setVoiceMode: (enabled: boolean) => void;
}

export const useChatStore = create<ChatState>((set) => ({
  messages: [],
  sessions: [],
  currentSessionId: "",
  networkStatus: "connecting",
  isRunning: false,
  voiceMode: false,

  setMessages: (messages) => set({ messages }),
  appendMessage: (message) => set((s) => ({ messages: [...s.messages, message] })),
  updateMessage: (id, updater) =>
    set((s) => ({
      messages: s.messages.map((m) => (m.id === id ? updater(m) : m)),
    })),
  setSessions: (sessions) => set({ sessions }),
  setCurrentSessionId: (id) => set({ currentSessionId: id }),
  setNetworkStatus: (status) => set({ networkStatus: status }),
  setIsRunning: (running) => set({ isRunning: running }),
  setVoiceMode: (voiceMode) => set({ voiceMode }),
}));
