import type { ModelInfo } from "@/SyscityWebSocketTransport";
import { Section } from "@/components/ui/Section";
import { Select } from "@/components/ui/Select";
import { Input } from "@/components/ui/Input";

interface AgentRegistryItem {
  id: string;
  display_name: string;
  emoji?: string;
  is_valid: boolean;
  has_heartbeat: boolean;
}

interface AgentDetail {
  agent_id: string;
  busy: boolean;
  status: string;
  config: Record<string, unknown> | null;
  personality: Record<string, unknown> | null;
}

interface AgentsSettingsProps {
  agentRegistry: AgentRegistryItem[];
  selectedAgentId: string;
  onSelectAgent: (id: string) => void;
  selectedAgentDetail: AgentDetail | null;
  agentDetailLoading: boolean;
  defaultAgent: Record<string, unknown>;
  models: ModelInfo[];
  agentModels: Record<string, string>;
  update: (path: string, value: unknown) => Promise<void>;
}

export function AgentsSettings({
  agentRegistry,
  selectedAgentId,
  onSelectAgent,
  selectedAgentDetail,
  agentDetailLoading,
  defaultAgent,
  models,
  agentModels,
  update,
}: AgentsSettingsProps) {
  return (
    <div className="space-y-5">
      <Section title="Select Agent">
        {agentRegistry.length === 0 ? (
          <div className="text-sm text-secondary">No agents in registry.</div>
        ) : (
          <Select
            value={selectedAgentId}
            onChange={(e) => onSelectAgent(e.target.value)}
          >
            {agentRegistry.map((a) => (
              <option key={a.id} value={a.id}>
                {a.display_name || a.id}
              </option>
            ))}
          </Select>
        )}
      </Section>

      <Section title="Model">
        <Select
          value={agentModels[selectedAgentId] ?? ""}
          onChange={(e) => update(`agent_models.${selectedAgentId}`, e.target.value || null)}
        >
          <option value="">Global default</option>
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.provider_name || m.provider} - {m.name}
            </option>
          ))}
        </Select>
        <div className="mt-1 text-[11px] text-secondary/70">
          Sessions for "{selectedAgentId}" use this model unless a session overrides it. Empty = global default.
        </div>
      </Section>

      {agentDetailLoading && (
        <div className="text-sm text-secondary flex items-center gap-2">
          <div className="w-4 h-4 border-2 border-subtle border-t-primary-500 rounded-full animate-spin" />
          Loading agent details...
        </div>
      )}

      {selectedAgentDetail && !agentDetailLoading && (
        <section className="space-y-4">
          {/* Agent header */}
          <div className="flex items-center gap-3">
            <span className="text-sm font-medium text-primary">
              {selectedAgentDetail.personality
                ? String((selectedAgentDetail.personality as Record<string, unknown>).display_name ?? selectedAgentDetail.agent_id)
                : selectedAgentDetail.agent_id}
            </span>
            <span className="text-xs text-secondary/70 font-mono">({selectedAgentDetail.agent_id})</span>
          </div>

          {/* Config */}
          {selectedAgentDetail.config && (
            <div>
              <h4 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Configuration</h4>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="px-3 py-2 rounded-lg bg-card">
                  <div className="text-[10px] uppercase tracking-wider text-secondary/70">Temperature</div>
                  <div className="text-sm text-primary">{typeof (selectedAgentDetail.config as Record<string, unknown>).temperature === "number" ? ((selectedAgentDetail.config as Record<string, unknown>).temperature as number).toFixed(2) : "—"}</div>
                </div>
                <div className="px-3 py-2 rounded-lg bg-card">
                  <div className="text-[10px] uppercase tracking-wider text-secondary/70">Max Tokens</div>
                  <div className="text-sm text-primary">{String((selectedAgentDetail.config as Record<string, unknown>).max_tokens ?? "—")}</div>
                </div>
                <div className="px-3 py-2 rounded-lg bg-card">
                  <div className="text-[10px] uppercase tracking-wider text-secondary/70">Max Turns</div>
                  <div className="text-sm text-primary">{String((selectedAgentDetail.config as Record<string, unknown>).max_turns ?? "—")}</div>
                </div>
                <div className="px-3 py-2 rounded-lg bg-card">
                  <div className="text-[10px] uppercase tracking-wider text-secondary/70">Max Concurrent Tools</div>
                  <div className="text-sm text-primary">{String((selectedAgentDetail.config as Record<string, unknown>).max_concurrent_tools ?? "—")}</div>
                </div>
              </div>
              {"workspace_only" in (selectedAgentDetail.config as Record<string, unknown>) && (
                <div className="mt-2 px-3 py-2 rounded-lg bg-card flex items-center justify-between">
                  <span className="text-sm text-secondary">Workspace Only</span>
                  <span className={`text-xs px-2 py-0.5 rounded-full ${(selectedAgentDetail.config as Record<string, unknown>).workspace_only ? "bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400" : "bg-sidebar text-secondary"}`}>
                    {(selectedAgentDetail.config as Record<string, unknown>).workspace_only ? "Yes" : "No"}
                  </span>
                </div>
              )}
            </div>
          )}

          {/* Personality */}
          {selectedAgentDetail.personality && (
            <div>
              <h4 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Personality</h4>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="px-3 py-2 rounded-lg bg-card">
                  <div className="text-[10px] uppercase tracking-wider text-secondary/70">Display Name</div>
                  <div className="text-sm text-primary">{String((selectedAgentDetail.personality as Record<string, unknown>).display_name ?? "—")}</div>
                </div>
                <div className="px-3 py-2 rounded-lg bg-card">
                  <div className="text-[10px] uppercase tracking-wider text-secondary/70">Valid</div>
                  <div className="text-sm text-primary">{(selectedAgentDetail.personality as Record<string, unknown>).is_valid ? "Yes" : "No"}</div>
                </div>
              </div>
              <div className="mt-2 flex flex-wrap gap-1.5">
                {Boolean((selectedAgentDetail.personality as Record<string, unknown>).has_heartbeat) && (
                  <span className="text-xs px-2 py-0.5 rounded-full bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400">Heartbeat</span>
                )}
                {Boolean((selectedAgentDetail.personality as Record<string, unknown>).has_soul) && (
                  <span className="text-xs px-2 py-0.5 rounded-full bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-400">Soul</span>
                )}
                {Boolean((selectedAgentDetail.personality as Record<string, unknown>).has_identity) && (
                  <span className="text-xs px-2 py-0.5 rounded-full bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-400">Identity</span>
                )}
                {Boolean((selectedAgentDetail.personality as Record<string, unknown>).has_memory) && (
                  <span className="text-xs px-2 py-0.5 rounded-full bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400">Memory</span>
                )}
              </div>
            </div>
          )}
        </section>
      )}

      <section>
        {(() => {
          const hasAgentCfg = selectedAgentDetail?.config != null;
          const ac = (selectedAgentDetail?.config as Record<string, unknown> | null) ?? defaultAgent;
          return (
            <>
              <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">
                {hasAgentCfg ? `${selectedAgentDetail!.agent_id} Parameters` : "Global Default Parameters"}
              </h3>
              {hasAgentCfg && (
                <div className="text-[11px] text-secondary/70 mb-2">Editing individual agent parameters is not yet supported. Changes here affect the global default.</div>
              )}
              <div className="space-y-3">
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <div>
                    <label className="block text-sm text-secondary mb-1">Temperature</label>
                    <div className="flex items-center gap-2">
                      <input type="range" min="0" max="2" step="0.1" value={(ac.temperature as number | undefined) ?? 0.7} onChange={(e) => update("default_agent.temperature", parseFloat(e.target.value))} className="flex-1 h-1.5 bg-secondary/20 dark:bg-secondary/20 rounded-lg appearance-none cursor-pointer accent-primary-500" />
                      <span className="text-sm text-secondary w-10 text-right tabular-nums">{((ac.temperature as number | undefined) ?? 0.7).toFixed(2)}</span>
                    </div>
                  </div>
                  <Input
                    label="Max Tokens"
                    labelClassName="block text-sm text-secondary mb-1"
                    type="number"
                    value={(ac.max_tokens as number | undefined) ?? 2048}
                    onChange={(e) => update("default_agent.max_tokens", parseInt(e.target.value))}
                  />
                </div>
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <Input
                    label="Max Turns"
                    labelClassName="block text-sm text-secondary mb-1"
                    type="number"
                    value={(ac.max_turns as number | undefined) ?? ""}
                    placeholder="Unlimited"
                    onChange={(e) => update("default_agent.max_turns", e.target.value ? parseInt(e.target.value) : null)}
                  />
                  <Input
                    label="Max Concurrent Tools"
                    labelClassName="block text-sm text-secondary mb-1"
                    type="number"
                    value={(ac.max_concurrent_tools as number | undefined) ?? 5}
                    onChange={(e) => update("default_agent.max_concurrent_tools", parseInt(e.target.value))}
                  />
                </div>
                <div>
                  <label className="block text-sm text-secondary mb-1">System Prompt</label>
                  <textarea value={(ac.system_prompt as string | undefined) || ""} onChange={(e) => update("default_agent.system_prompt", e.target.value)} className="w-full h-[60vh] rounded-lg border border-subtle bg-card px-3 py-2 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20 resize-none font-mono" />
                </div>
              </div>
            </>
          );
        })()}
      </section>
    </div>
  );
}
