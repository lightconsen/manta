import { create } from "zustand";
import type { ChatMessage, NetworkStatus } from "@/SyscityWebSocketTransport";

const INTERNALS_KEY = "syscity_internals_visibility";

function loadInternalsVisibility(): Record<string, boolean> {
  try {
    return JSON.parse(localStorage.getItem(INTERNALS_KEY) || "{}");
  } catch {
    return {};
  }
}

function saveInternalsVisibility(v: Record<string, boolean>): void {
  try {
    localStorage.setItem(INTERNALS_KEY, JSON.stringify(v));
  } catch { /* quota exceeded */ }
}

interface ChatState {
  messages: ChatMessage[];
  sessions: Array<{
    id: string;
    label?: string;
    agent_id?: string;
    pinned?: boolean;
    model?: string | null;
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
  runningSessionIds: string[];
  voiceMode: boolean;
  isLoadingHistory: boolean;
  hasMoreHistory: boolean;
  aiInternalsVisibility: Record<string, boolean>;
  previewDocument: { filename: string; title: string; format: string } | null;

  setMessages: (messages: ChatMessage[]) => void;
  prependMessages: (messages: ChatMessage[]) => void;
  appendMessage: (message: ChatMessage) => void;
  updateMessage: (id: string, updater: (msg: ChatMessage) => ChatMessage) => void;
  setSessions: (
    sessions: Array<{
      id: string;
      label?: string;
      agent_id?: string;
      pinned?: boolean;
      model?: string | null;
      last_activity?: number;
    }>
  ) => void;
  setCurrentSessionId: (id: string) => void;
  setCurrentAgent: (agent?: { id: string; display_name: string; emoji: string }) => void;
  setNetworkStatus: (status: NetworkStatus) => void;
  setIsRunning: (running: boolean) => void;
  setRunningSessionIds: (ids: string[]) => void;
  setVoiceMode: (enabled: boolean) => void;
  setIsLoadingHistory: (loading: boolean) => void;
  setHasMoreHistory: (hasMore: boolean) => void;
  setAiInternalsVisibility: (messageId: string, visible: boolean) => void;
  setPreviewDocument: (doc: { filename: string; title: string; format: string } | null) => void;
}

export const useChatStore = create<ChatState>((set) => ({
  messages: [],
  sessions: [],
  currentSessionId: "",
  networkStatus: "connecting",
  isRunning: false,
  runningSessionIds: [],
  voiceMode: false,
  isLoadingHistory: false,
  hasMoreHistory: false,
  aiInternalsVisibility: loadInternalsVisibility(),
  previewDocument: null,

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
  setRunningSessionIds: (ids) => set({ runningSessionIds: ids }),
  setVoiceMode: (voiceMode) => set({ voiceMode }),
  setIsLoadingHistory: (isLoadingHistory) => set({ isLoadingHistory }),
  setHasMoreHistory: (hasMoreHistory) => set({ hasMoreHistory }),
  setAiInternalsVisibility: (messageId, visible) =>
    set((s) => {
      const next = {
        ...s.aiInternalsVisibility,
        [messageId]: visible,
      };
      saveInternalsVisibility(next);
      return { aiInternalsVisibility: next };
    }),
  setPreviewDocument: (doc) => set({ previewDocument: doc }),
}));
