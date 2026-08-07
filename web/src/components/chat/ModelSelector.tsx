import { useEffect, useState } from "react";
import type { ModelInfo, SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";

interface ModelSelectorProps {
  transport: SyscityWebSocketTransport;
}

// Compact model picker for the chat composer. Shows the effective model for
// the active session (explicit session pin -> bound agent's model -> global
// default) and persists a session-level pin via sessions.set_model on change.
export function ModelSelector({ transport }: ModelSelectorProps) {
  const [sessionId, setSessionId] = useState(() => transport.getSessionId());
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [defaultModel, setDefaultModel] = useState("");
  const [agentModels, setAgentModels] = useState<Record<string, string>>({});
  const [sessionModel, setSessionModel] = useState<string | null>(null);
  const [sessionAgentId, setSessionAgentId] = useState("");

  // Track the active session.
  useEffect(
    () => transport.onSessionChange(() => setSessionId(transport.getSessionId())),
    [transport]
  );

  // Load the model list and per-agent bindings once.
  useEffect(() => {
    let cancelled = false;
    transport
      .listModels()
      .then((r) => {
        if (cancelled) return;
        setModels(r.models);
        setDefaultModel(r.default_model);
      })
      .catch(() => {});
    transport
      .getConfig()
      .then((c) => {
        if (cancelled) return;
        setAgentModels((c.agent_models as Record<string, string>) || {});
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [transport]);

  // Load the current session's pin + bound agent whenever the session changes.
  useEffect(() => {
    let cancelled = false;
    transport
      .listSessions()
      .then((list) => {
        if (cancelled) return;
        const s = list.find((x) => x.id === sessionId);
        setSessionModel(s?.model ?? null);
        setSessionAgentId(s?.agent_id ?? "");
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [transport, sessionId]);

  // React to model changes for this session (from this or other clients).
  useEffect(
    () =>
      transport.onEvent((evt) => {
        if (evt.event !== "session.model_changed") return;
        const p = evt.payload as { session_id?: string; model?: string | null } | undefined;
        if (p?.session_id === sessionId) {
          setSessionModel(p.model ?? null);
        }
      }),
    [transport, sessionId]
  );

  const effective = sessionModel ?? agentModels[sessionAgentId] ?? defaultModel;
  // What the model would be if the session pin were cleared (agent binding ->
  // global default). Shown on the "clear pin" option so it labels what you get
  // by switching back, not the currently pinned model.
  const fallback = agentModels[sessionAgentId] ?? defaultModel;

  // Group models by provider for the option list.
  const byProvider = new Map<string, ModelInfo[]>();
  for (const m of models) {
    const list = byProvider.get(m.provider) || [];
    list.push(m);
    byProvider.set(m.provider, list);
  }

  const handleChange = (value: string) => {
    const m = value === "" ? null : value;
    setSessionModel(m); // optimistic; the model_changed event reconciles
    transport.setSessionModel(sessionId, m).catch(() => {});
  };

  if (models.length === 0) return null;

  return (
    <select
      value={sessionModel ?? ""}
      onChange={(e) => handleChange(e.target.value)}
      title={`Model: ${effective || "default"}${sessionModel ? "" : " (default)"}`}
      aria-label="Select model"
      className="max-w-[10rem] truncate self-center rounded-lg border border-subtle bg-card px-2 py-1.5 text-xs text-secondary hover:text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20 transition"
    >
      <option value="">
        {fallback ? `${fallback} (default)` : "Default model"}
      </option>
      {Array.from(byProvider.entries()).map(([provider, ms]) => (
        <optgroup key={provider} label={ms[0]?.provider_name || provider}>
          {ms.map((m) => (
            <option key={m.id} value={m.id}>
              {m.provider_name} - {m.name}
            </option>
          ))}
        </optgroup>
      ))}
    </select>
  );
}
