import { create } from "zustand";

export interface GoalState {
  id: string;
  description: string;
  conditions: string[];
  maxRounds: number;
  round: number;
  passed: number;
  total: number;
  status: "running" | "done" | "aborted";
  summary?: string;
  reason?: string;
}

interface GoalStoreState {
  goals: Record<string, GoalState>;
  updateGoal: (id: string, updates: Partial<GoalState>) => void;
  removeGoal: (id: string) => void;
}

export const useGoalStore = create<GoalStoreState>((set) => ({
  goals: {},

  updateGoal: (id, updates) =>
    set((s) => {
      const prev = s.goals[id];
      if (!prev) return s;
      return {
        goals: { ...s.goals, [id]: { ...prev, ...updates } },
      };
    }),

  removeGoal: (id) =>
    set((s) => {
      const { [id]: _, ...rest } = s.goals;
      return { goals: rest };
    }),
}));
