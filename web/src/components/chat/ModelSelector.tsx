import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import type { ModelInfo, SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { getProviderLogo } from "@/lib/providerLogos";

interface ModelSelectorProps {
  transport: SyscityWebSocketTransport;
}

/** Provider logo (or a first-letter fallback for unknown providers). */
function ProviderLogo({ provider, providerName }: { provider: string; providerName: string }) {
  const logo = getProviderLogo(provider);
  if (logo) {
    return <img src={logo} alt="" className="w-4 h-4 object-contain shrink-0" />;
  }
  return (
    <span className="w-4 h-4 shrink-0 rounded bg-sidebar flex items-center justify-center text-[9px] font-semibold text-secondary">
      {(providerName || provider).charAt(0).toUpperCase()}
    </span>
  );
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
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

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

  // Close the dropdown on outside click or Escape.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const effective = sessionModel ?? agentModels[sessionAgentId] ?? defaultModel;
  // What the model would be if the session pin were cleared (agent binding ->
  // global default). Shown on the "clear pin" option so it labels what you get
  // by switching back, not the currently pinned model.
  const fallback = agentModels[sessionAgentId] ?? defaultModel;
  const effectiveModel = models.find((m) => m.id === effective) ?? null;

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
    setOpen(false);
  };

  if (models.length === 0) return null;

  return (
    <div ref={rootRef} className="relative self-center">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        // Don't take focus on mouse click so the composer's focus-within ring
        // (and the button's own ring) never highlight when picking a model.
        onMouseDown={(e) => e.preventDefault()}
        title={`Model: ${effective || "default"}${sessionModel ? "" : " (default)"}`}
        aria-label="Select model"
        aria-haspopup="listbox"
        aria-expanded={open}
        className="flex items-center gap-1.5 max-w-[10rem] rounded-lg border border-subtle bg-card px-2 py-1.5 text-xs text-secondary hover:text-primary focus:outline-none transition"
      >
        {effectiveModel ? (
          <ProviderLogo provider={effectiveModel.provider} providerName={effectiveModel.provider_name} />
        ) : (
          <span className="w-4 h-4 shrink-0 rounded bg-sidebar flex items-center justify-center text-[9px] font-semibold text-secondary">
            {(effective || "D").charAt(0).toUpperCase()}
          </span>
        )}
        <span className="truncate">
          {effectiveModel
            ? `${effectiveModel.provider_name} - ${effectiveModel.name}`
            : effective || "Default model"}
        </span>
        <ChevronDown
          className={`w-3 h-3 shrink-0 text-secondary/60 transition-transform ${open ? "rotate-180" : ""}`}
        />
      </button>

      {open && (
        <div
          role="listbox"
          className="absolute bottom-full left-0 mb-1.5 w-64 bg-card rounded-xl shadow-xl border border-subtle overflow-hidden z-50"
        >
          <div className="max-h-72 overflow-y-auto py-1">
            {/* Clear-pin option */}
            <button
              type="button"
              role="option"
              aria-selected={sessionModel === null}
              onClick={() => handleChange("")}
              className={`w-full flex items-center gap-2 px-3 py-2 text-left transition ${
                sessionModel === null
                  ? "bg-primary-50 dark:bg-primary-900/20"
                  : "hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
              }`}
            >
              <span className="w-4 h-4 shrink-0" />
              <span className="flex-1 min-w-0">
                <span className="block text-sm text-primary">Default model</span>
                {fallback && (
                  <span className="block text-xs text-secondary truncate">uses {fallback}</span>
                )}
              </span>
              {sessionModel === null && <Check className="w-4 h-4 text-primary shrink-0" />}
            </button>

            {Array.from(byProvider.entries()).map(([provider, ms]) => (
              <div key={provider}>
                <div className="px-3 pt-2 pb-1 text-[10px] uppercase tracking-wide text-secondary/70">
                  {ms[0]?.provider_name || provider}
                </div>
                {ms.map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    role="option"
                    aria-selected={sessionModel === m.id}
                    onClick={() => handleChange(m.id)}
                    className={`w-full flex items-center gap-2 px-3 py-2 text-left transition ${
                      sessionModel === m.id
                        ? "bg-primary-50 dark:bg-primary-900/20"
                        : "hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
                    }`}
                  >
                    <ProviderLogo provider={m.provider} providerName={m.provider_name} />
                    <span className="flex-1 min-w-0 truncate text-sm text-primary">
                      {m.provider_name} - {m.name}
                    </span>
                    {sessionModel === m.id && <Check className="w-4 h-4 text-primary shrink-0" />}
                  </button>
                ))}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
