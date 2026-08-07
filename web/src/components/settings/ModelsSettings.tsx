import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import type { SyscityConfig } from "@/components/settings/useSettingsData";
import { AddModelForm } from "@/components/settings/AddModelForm";
import { Section } from "@/components/ui/Section";
import { Button } from "@/components/ui/Button";

interface ModelsSettingsProps {
  transport: SyscityWebSocketTransport;
  models: Array<{ id: string; name: string; provider: string }>;
  config: SyscityConfig;
  modelActionLoading: string;
  showAddModel: boolean;
  onToggleAdd: () => void;
  onRefresh: () => Promise<void>;
  onSetDefault: (id: string) => Promise<void>;
  onRemove: (id: string) => Promise<void>;
}

export function ModelsSettings({
  transport,
  models,
  config,
  modelActionLoading,
  showAddModel,
  onToggleAdd,
  onRefresh,
  onSetDefault,
  onRemove,
}: ModelsSettingsProps) {
  return (
    <div className="space-y-5">
      <Section
        title="Available Models"
        right={
          <Button variant="primary-sm" onClick={onToggleAdd}>
            {showAddModel ? "Cancel" : "+ Add"}
          </Button>
        }
      >
        {showAddModel && (
          <div className="mb-4">
            <AddModelForm transport={transport} onAdded={onRefresh} />
          </div>
        )}

        {models.length === 0 ? (
          <div className="text-sm text-secondary">No models available.</div>
        ) : (
          <div className="space-y-2">
            {models.map((m) => (
              <div key={m.id} className="flex items-center justify-between px-3 py-2 rounded-lg bg-card">
                <div className="flex items-center gap-2">
                  <span className="text-sm text-primary font-medium">{m.name}</span>
                  <span className="text-xs text-secondary">{m.provider}</span>
                </div>
                <div className="flex items-center gap-2">
                  {config.model === m.id ? (
                    <span className="text-xs px-2 py-0.5 rounded-full bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400">Default</span>
                  ) : (
                    <button
                      onClick={() => onSetDefault(m.id)}
                      disabled={modelActionLoading === `default_${m.id}`}
                      className="text-xs px-2 py-0.5 rounded-full bg-sidebar text-secondary hover:bg-primary-100 dark:hover:bg-primary-900/30 hover:text-primary-700 dark:hover:text-primary-400 transition"
                    >
                      {modelActionLoading === `default_${m.id}` ? "..." : "Set Default"}
                    </button>
                  )}
                  <button
                    onClick={() => onRemove(m.id)}
                    disabled={modelActionLoading === m.id}
                    className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-secondary/60 hover:text-red-600 dark:hover:text-red-400 transition"
                    title="Remove"
                  >
                    <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <line x1="18" y1="6" x2="6" y2="18" />
                      <line x1="6" y1="6" x2="18" y2="18" />
                    </svg>
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </Section>
    </div>
  );
}
